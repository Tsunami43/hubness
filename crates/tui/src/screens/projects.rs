//! Projects (the web app's projects page): groups on the left, projects on the right with
//! language, git host, folder and last update; search, group / language filters and sort.
//! New project: clone by URL (a terminal job shown here), an empty repository, or import
//! repositories already on the server (also from another folder). New group with picked
//! members. Removing a project, optionally with its folder; missing folders are marked.

use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph, Wrap};
use serde_json::{Value, json};

use super::project::enc;
use crate::core::{Action, Ctx, Handled, Route, Screen};
use crate::ui::term::JobTerm;
use crate::ui::{Theme, ago, block, centered};

/// `#rrggbb` → a terminal color.
pub fn hex_color(hex: &str) -> Option<Color> {
    let h = hex.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(h, 16).ok()?;
    Some(Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

/// The folder name `git clone` would make for a URL.
pub fn derived_name(url: &str) -> String {
    let u = url.trim().trim_end_matches('/');
    let last = u.rsplit(['/', ':']).next().unwrap_or_default();
    last.strip_suffix(".git").unwrap_or(last).to_string()
}

/// owner/name of a remote URL (`git@host:owner/name.git`, `https://host/owner/name`).
pub fn slug_of(url: &str) -> String {
    let u = url.trim_end_matches('/').trim_end_matches(".git");
    let parts: Vec<&str> = u.split(['/', ':']).filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        [.., owner, name] => format!("{owner}/{name}"),
        _ => u.to_string(),
    }
}

fn basename(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path)
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Focus {
    Groups,
    Projects,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Sort {
    Updated,
    Name,
}

/// Import: repositories found in a folder, checked ones are imported.
struct Import {
    root: String,
    repos: Vec<Value>,
    picked: Vec<String>,
    list: ListState,
    filter: String,
    loading: bool,
    error: Option<String>,
    /// Browsing server folders for another place to look (`/fs/browse`).
    browse: Option<(Value, ListState)>,
}

impl Import {
    fn visible(&self) -> Vec<&Value> {
        let needle = self.filter.to_lowercase();
        self.repos.iter().filter(|r| r["path"].as_str().unwrap_or_default().to_lowercase().contains(&needle)).collect()
    }
}

/// New group: name, description and folder are asked first, then members are picked.
struct NewGroup {
    name: String,
    description: String,
    folder: String,
    picked: HashSet<String>,
    list: ListState,
    filter: String,
}

pub struct Projects {
    projects: Vec<Value>,
    groups: Vec<Value>,
    focus: Focus,
    plist: ListState,
    glist: ListState,
    filter: String,
    group_filter: Option<String>,
    lang_filter: Option<String>,
    sort: Sort,
    import: Option<Import>,
    group: Option<NewGroup>,
    job: Option<JobTerm>,
    clone_url: String,
    term_size: (u16, u16),
}

impl Default for Projects {
    fn default() -> Self {
        Self {
            projects: vec![],
            groups: vec![],
            focus: Focus::Projects,
            plist: ListState::default(),
            glist: ListState::default(),
            filter: String::new(),
            group_filter: None,
            lang_filter: None,
            sort: Sort::Updated,
            import: None,
            group: None,
            job: None,
            clone_url: String::new(),
            term_size: (100, 24),
        }
    }
}

impl Projects {
    fn reload(&self, ctx: &mut Ctx) {
        ctx.get("projects", "/projects");
        ctx.get("groups", "/groups");
    }

    fn shown(&self) -> Vec<&Value> {
        let needle = self.filter.trim().to_lowercase();
        let mut list: Vec<&Value> = self
            .projects
            .iter()
            .filter(|p| {
                let hay = format!(
                    "{} {} {} {}",
                    p["name"].as_str().unwrap_or_default(),
                    p["description"].as_str().unwrap_or_default(),
                    p["path"].as_str().unwrap_or_default(),
                    p["repoUrl"].as_str().unwrap_or_default()
                )
                .to_lowercase();
                needle.is_empty() || hay.contains(&needle)
            })
            .filter(|p| self.group_filter.as_ref().is_none_or(|g| p["groupIds"].as_array().is_some_and(|ids| ids.iter().any(|i| i == g))))
            .filter(|p| self.lang_filter.as_ref().is_none_or(|l| p["language"]["name"].as_str() == Some(l)))
            .collect();
        match self.sort {
            Sort::Name => list.sort_by_key(|p| p["name"].as_str().unwrap_or_default().to_lowercase()),
            Sort::Updated => list.sort_by(|a, b| b["updatedAt"].as_str().cmp(&a["updatedAt"].as_str())),
        }
        list
    }

    fn selected_project(&self) -> Option<Value> {
        self.plist.selected().and_then(|i| self.shown().get(i).map(|p| (*p).clone()))
    }

    fn group_name(&self, id: &str) -> String {
        self.groups.iter().find(|g| g["id"] == id).and_then(|g| g["name"].as_str()).unwrap_or("?").to_string()
    }

    fn project_name(&self, id: &str) -> String {
        self.projects.iter().find(|p| p["id"] == id).and_then(|p| p["name"].as_str()).unwrap_or("?").to_string()
    }

    fn storage(ctx: &Ctx) -> String {
        ctx.shared.settings["projectsRoot"].as_str().unwrap_or_default().to_string()
    }

    fn discover(&mut self, path: Option<String>, ctx: &mut Ctx) {
        let q = path.as_deref().map(|p| format!("?path={}", enc(p))).unwrap_or_default();
        if let Some(im) = &mut self.import {
            im.loading = true;
            im.error = None;
        }
        ctx.get("discover", format!("/projects/discover{q}"));
    }

    fn browse(&self, path: Option<&str>, ctx: &mut Ctx) {
        let q = path.map(|p| format!("?path={}", enc(p))).unwrap_or_default();
        ctx.get("browse", format!("/fs/browse{q}"));
    }

    fn remove(&mut self, ctx: &mut Ctx) {
        let Some(p) = self.selected_project() else { return };
        let id = p["id"].as_str().unwrap_or_default();
        let name = p["name"].as_str().unwrap_or_default();
        if p["missing"] == true {
            ctx.confirm(
                &format!("remove:{id}"),
                "Remove the project",
                format!("Remove “{name}” from the hub? Its folder is already gone."),
                true,
            );
            return;
        }
        let path = p["path"].as_str().unwrap_or_default();
        ctx.pick(
            &format!("remove-how:{id}"),
            format!("Remove “{name}”"),
            vec![
                ("keep".into(), "Remove from the hub — the folder stays on disk".into()),
                ("files".into(), format!("Remove and delete the folder {path} for good")),
            ],
        );
    }

    fn move_list(list: &mut ListState, n: usize, d: isize) {
        if n == 0 {
            list.select(None);
            return;
        }
        let i = list.selected().unwrap_or(0) as isize + d;
        list.select(Some(i.clamp(0, n as isize - 1) as usize));
    }

    // ---------- overlays ----------

    fn import_key(&mut self, k: KeyEvent, ctx: &mut Ctx) {
        let Some(im) = &mut self.import else { return };
        if let Some((data, list)) = &mut im.browse {
            let parent = data["parent"].as_str().map(String::from);
            let entries: Vec<String> =
                data["entries"].as_array().into_iter().flatten().filter_map(|e| e["path"].as_str().map(String::from)).collect();
            let offset = usize::from(parent.is_some());
            let n = entries.len() + offset;
            match k.code {
                KeyCode::Esc => im.browse = None,
                KeyCode::Down | KeyCode::Char('j') => Self::move_list(list, n, 1),
                KeyCode::Up | KeyCode::Char('k') => Self::move_list(list, n, -1),
                KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                    let i = list.selected().unwrap_or(0);
                    let target = if parent.is_some() && i == 0 { parent } else { entries.get(i - offset).cloned() };
                    if let Some(t) = target {
                        self.browse(Some(&t), ctx);
                    }
                }
                KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => {
                    if let Some(p) = parent {
                        self.browse(Some(&p), ctx);
                    }
                }
                KeyCode::Char('g') => ctx.prompt("browse-path", "Go to folder", data["path"].as_str().unwrap_or_default()),
                KeyCode::Char(' ') | KeyCode::Char('s') => {
                    let path = data["path"].as_str().unwrap_or_default().to_string();
                    if data["isGitRepo"] == true {
                        if !im.picked.contains(&path) {
                            im.picked.push(path);
                        }
                        im.browse = None;
                    } else {
                        im.browse = None;
                        im.root = path.clone();
                        self.discover(Some(path), ctx);
                    }
                }
                _ => {}
            }
            return;
        }
        let visible: Vec<String> = im.visible().iter().filter_map(|r| r["path"].as_str().map(String::from)).collect();
        match k.code {
            KeyCode::Esc => self.import = None,
            KeyCode::Down | KeyCode::Char('j') => Self::move_list(&mut im.list, visible.len(), 1),
            KeyCode::Up | KeyCode::Char('k') => Self::move_list(&mut im.list, visible.len(), -1),
            KeyCode::Char(' ') => {
                if let Some(p) = im.list.selected().and_then(|i| visible.get(i)) {
                    match im.picked.iter().position(|x| x == p) {
                        Some(i) => {
                            im.picked.remove(i);
                        }
                        None => im.picked.push(p.clone()),
                    }
                }
            }
            KeyCode::Char('a') => {
                let all = visible.iter().all(|p| im.picked.contains(p));
                if all {
                    im.picked.retain(|p| !visible.contains(p));
                } else {
                    for p in visible {
                        if !im.picked.contains(&p) {
                            im.picked.push(p);
                        }
                    }
                }
            }
            KeyCode::Char('/') => ctx.prompt("import-filter", "Filter repositories", im.filter.clone()),
            KeyCode::Char('o') => {
                im.browse = Some((Value::Null, ListState::default()));
                let start = im.root.clone();
                self.browse(Some(&start), ctx);
            }
            KeyCode::Enter => {
                if im.picked.is_empty() {
                    ctx.error("check repositories with Space first");
                } else {
                    ctx.send("imported", Method::POST, "/projects/import", json!({ "paths": im.picked }));
                }
            }
            _ => {}
        }
    }

    fn group_key(&mut self, k: KeyEvent, ctx: &mut Ctx) {
        let ids: Vec<String> = self.member_candidates().iter().filter_map(|p| p["id"].as_str().map(String::from)).collect();
        let Some(g) = &mut self.group else { return };
        match k.code {
            KeyCode::Esc => self.group = None,
            KeyCode::Down | KeyCode::Char('j') => Self::move_list(&mut g.list, ids.len(), 1),
            KeyCode::Up | KeyCode::Char('k') => Self::move_list(&mut g.list, ids.len(), -1),
            KeyCode::Char(' ') => {
                if let Some(id) = g.list.selected().and_then(|i| ids.get(i))
                    && !g.picked.remove(id)
                {
                    g.picked.insert(id.clone());
                }
            }
            KeyCode::Char('/') => ctx.prompt("member-filter", "Filter projects", g.filter.clone()),
            KeyCode::Enter => {
                let folder = g.folder.trim();
                let members: Vec<&String> =
                    self.projects.iter().filter_map(|p| p["id"].as_str()).filter_map(|id| g.picked.get(id)).collect();
                ctx.send(
                    "group-created",
                    Method::POST,
                    "/groups",
                    json!({
                        "name": g.name,
                        "description": g.description,
                        "folder": if folder.is_empty() { Value::Null } else { json!(folder) },
                        "projectIds": members,
                    }),
                );
            }
            _ => {}
        }
    }

    fn member_candidates(&self) -> Vec<&Value> {
        let needle = self.group.as_ref().map(|g| g.filter.to_lowercase()).unwrap_or_default();
        self.projects
            .iter()
            .filter(|p| {
                format!("{} {}", p["name"].as_str().unwrap_or_default(), p["path"].as_str().unwrap_or_default())
                    .to_lowercase()
                    .contains(&needle)
            })
            .collect()
    }

    fn draw_import(&mut self, f: &mut Frame, area: Rect) {
        let t = Theme::get();
        let Some(im) = &mut self.import else { return };
        let r = centered(area, 100, area.height.saturating_sub(4).max(12));
        f.render_widget(Clear, r);
        if let Some((data, list)) = &mut im.browse {
            let b = block("Browse server folders — Enter open · h up · g go to · Space choose this folder · Esc back", true);
            let inner = b.inner(r);
            f.render_widget(b, r);
            let [head, body] = Layout::vertical([Constraint::Length(3), Constraint::Min(1)]).areas(inner);
            if data.is_null() {
                f.render_widget(Paragraph::new("loading…"), head);
                return;
            }
            let repo = if data["isGitRepo"] == true {
                "  — a git repository: Space imports it"
            } else {
                "  — Space looks for repositories here"
            };
            f.render_widget(
                Paragraph::new(vec![Line::from(vec![
                    Span::styled(data["path"].as_str().unwrap_or_default().to_string(), t.bold()),
                    Span::styled(repo, Style::default().fg(t.ok)),
                ])])
                .wrap(Wrap { trim: false }),
                head,
            );
            let mut items: Vec<ListItem> = vec![];
            if data["parent"].is_string() {
                items.push(ListItem::new(Line::styled("  ↑ ..", t.muted_style())));
            }
            for e in data["entries"].as_array().into_iter().flatten() {
                items.push(ListItem::new(Line::raw(format!("  ▸ {}", e["name"].as_str().unwrap_or_default()))));
            }
            if items.is_empty() {
                items.push(ListItem::new(Line::styled("  no subfolders", t.muted_style())));
            }
            if list.selected().is_none() {
                list.select(Some(0));
            }
            f.render_stateful_widget(List::new(items).highlight_style(t.selected()), body, list);
            return;
        }
        let title = format!(
            "Import repositories ({} checked) — Space check · a all · / filter · o other folder · Enter import · Esc cancel",
            im.picked.len()
        );
        let b = block(title, true);
        let inner = b.inner(r);
        f.render_widget(b, r);
        let extra: Vec<String> = im.picked.iter().filter(|p| !im.repos.iter().any(|r| r["path"] == p.as_str())).cloned().collect();
        let [head, body, foot] =
            Layout::vertical([Constraint::Length(3), Constraint::Min(1), Constraint::Length(extra.len().min(4) as u16)]).areas(inner);
        let mut head_lines = vec![Line::styled(
            format!("Repositories in {} not added to the hub yet (their files stay where they are)", im.root),
            t.muted_style(),
        )];
        if !im.filter.is_empty() {
            head_lines.push(Line::styled(format!("filter: {}", im.filter), t.muted_style()));
        }
        f.render_widget(Paragraph::new(head_lines).wrap(Wrap { trim: false }), head);
        if let Some(e) = &im.error {
            f.render_widget(Paragraph::new(Line::styled(e.clone(), Style::default().fg(t.danger))), body);
        } else if im.loading {
            f.render_widget(Paragraph::new(Line::styled("looking for repositories…", t.muted_style())), body);
        } else {
            let root = im.root.trim_end_matches('/').to_string();
            let items: Vec<ListItem> = im
                .visible()
                .iter()
                .map(|r| {
                    let p = r["path"].as_str().unwrap_or_default();
                    let on = im.picked.iter().any(|x| x == p);
                    let rel = p.strip_prefix(&format!("{root}/")).unwrap_or(p).to_string();
                    ListItem::new(Line::from(vec![
                        Span::styled(if on { " [x] " } else { " [ ] " }, if on { t.accent_style() } else { t.muted_style() }),
                        Span::raw(rel),
                    ]))
                })
                .collect();
            if items.is_empty() {
                f.render_widget(
                    Paragraph::new(Line::styled("No new repositories here — o looks in another folder.", t.muted_style())),
                    body,
                );
            } else {
                if im.list.selected().is_none() {
                    im.list.select(Some(0));
                }
                f.render_stateful_widget(List::new(items).highlight_style(t.selected()), body, &mut im.list);
            }
        }
        let extra_lines: Vec<Line> = extra.iter().take(4).map(|p| Line::styled(format!(" [x] {p}"), t.accent_style())).collect();
        f.render_widget(Paragraph::new(extra_lines), foot);
    }

    fn draw_group(&mut self, f: &mut Frame, area: Rect) {
        let t = Theme::get();
        let rows: Vec<(String, String, String)> = self
            .member_candidates()
            .iter()
            .map(|p| {
                let path = p["path"].as_str().unwrap_or_default();
                let short: Vec<&str> = path.rsplit('/').take(2).collect();
                (
                    p["id"].as_str().unwrap_or_default().to_string(),
                    p["name"].as_str().unwrap_or_default().to_string(),
                    short.into_iter().rev().collect::<Vec<_>>().join("/"),
                )
            })
            .collect();
        let Some(g) = &mut self.group else { return };
        let r = centered(area, 90, area.height.saturating_sub(4).max(12));
        f.render_widget(Clear, r);
        let b = block(
            format!("New group “{}” — members ({}) · Space check · / filter · Enter create · Esc cancel", g.name, g.picked.len()),
            true,
        );
        let inner = b.inner(r);
        f.render_widget(b, r);
        let [head, body] = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(inner);
        let folder = if g.folder.trim().is_empty() { "the shared projects folder".to_string() } else { g.folder.clone() };
        f.render_widget(
            Paragraph::new(vec![
                Line::styled(format!("New projects of the group go to {folder}"), t.muted_style()),
                Line::styled(if g.filter.is_empty() { String::new() } else { format!("filter: {}", g.filter) }, t.muted_style()),
            ]),
            head,
        );
        let items: Vec<ListItem> = rows
            .iter()
            .map(|(id, name, path)| {
                let on = g.picked.contains(id);
                ListItem::new(Line::from(vec![
                    Span::styled(if on { " [x] " } else { " [ ] " }, if on { t.accent_style() } else { t.muted_style() }),
                    Span::raw(name.clone()),
                    Span::styled(format!("  {path}"), t.muted_style()),
                ]))
            })
            .collect();
        if g.list.selected().is_none() && !items.is_empty() {
            g.list.select(Some(0));
        }
        f.render_stateful_widget(List::new(items).highlight_style(t.selected()), body, &mut g.list);
    }

    fn draw_job(&mut self, f: &mut Frame, area: Rect) {
        let Some(job) = &mut self.job else { return };
        let r = centered(area, area.width.saturating_sub(4), area.height.saturating_sub(2));
        self.term_size = JobTerm::size_for(r);
        f.render_widget(Clear, r);
        let state = if job.running {
            "running — keys go to the command · Ctrl+] hide".to_string()
        } else {
            format!("finished (exit {}) — Esc / Enter close", job.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "?".into()))
        };
        let title = format!("{} — {state}", job.title);
        job.draw(f, r, block(title, true));
    }
}

impl Screen for Projects {
    fn title(&self) -> String {
        "Projects".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        self.reload(ctx);
    }

    fn captures_text(&self) -> bool {
        self.job.is_some() || self.import.is_some() || self.group.is_some()
    }

    fn actions(&self) -> Vec<Action> {
        if self.captures_text() {
            return vec![];
        }
        let sort = if self.sort == Sort::Updated { "Sort by name" } else { "Sort by last update" };
        vec![
            Action::new("n", "New project (clone / empty / import)"),
            Action::new("c", "Clone a repository by URL"),
            Action::new("e", "New empty repository"),
            Action::new("i", "Import repositories from the server"),
            Action::new("G", "New group"),
            Action::new("x", "Remove the selected project"),
            Action::new("/", "Find projects"),
            Action::new("g", "Filter by group"),
            Action::new("l", "Filter by language"),
            Action::new("s", sort),
        ]
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        match key {
            "n" => ctx.pick(
                "new-mode",
                "New project",
                vec![
                    ("clone".into(), "Clone a repository by URL".into()),
                    ("empty".into(), "New empty repository".into()),
                    ("import".into(), "Import repositories already on the server".into()),
                ],
            ),
            "c" => ctx.prompt("clone-url", "Repository URL (e.g. git@github.com:acme/app.git)", ""),
            "e" => ctx.prompt("new-repo", format!("New empty repository — name (in {})", Self::storage(ctx)), ""),
            "i" => {
                self.import = Some(Import {
                    root: Self::storage(ctx),
                    repos: vec![],
                    picked: vec![],
                    list: ListState::default(),
                    filter: String::new(),
                    loading: true,
                    error: None,
                    browse: None,
                });
                self.discover(None, ctx);
            }
            "G" => ctx.prompt("group-name", "New group — name", ""),
            "x" => self.remove(ctx),
            "/" => ctx.prompt("filter", "Find projects", self.filter.clone()),
            "g" => {
                let mut items = vec![(String::new(), "All groups".to_string())];
                items.extend(
                    self.groups
                        .iter()
                        .map(|g| (g["id"].as_str().unwrap_or_default().to_string(), g["name"].as_str().unwrap_or_default().to_string())),
                );
                ctx.pick("group-filter", "Filter by group", items);
            }
            "l" => {
                let mut langs: Vec<String> =
                    self.projects.iter().filter_map(|p| p["language"]["name"].as_str().map(String::from)).collect();
                langs.sort();
                langs.dedup();
                let mut items = vec![(String::new(), "All languages".to_string())];
                items.extend(langs.into_iter().map(|l| (l.clone(), l)));
                ctx.pick("lang-filter", "Filter by language", items);
            }
            "s" => self.sort = if self.sort == Sort::Updated { Sort::Name } else { Sort::Updated },
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        if let Some(job) = &mut self.job {
            let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
            if job.running {
                if ctrl && k.code == KeyCode::Char(']') {
                    self.job = None;
                    self.reload(ctx);
                } else if let Some(data) = JobTerm::key_bytes(k) {
                    ctx.send("job-input", Method::POST, format!("/jobs/{}/input", job.id), json!({ "data": data }));
                }
            } else if matches!(k.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                let id = job.id.clone();
                self.job = None;
                ctx.close_stream(&format!("job:{id}"));
                self.reload(ctx);
            }
            return Handled::Yes;
        }
        if self.import.is_some() {
            self.import_key(k, ctx);
            return Handled::Yes;
        }
        if self.group.is_some() {
            self.group_key(k, ctx);
            return Handled::Yes;
        }
        let n = self.shown().len();
        match (self.focus, k.code) {
            (_, KeyCode::Tab) | (Focus::Groups, KeyCode::Right) | (Focus::Projects, KeyCode::Left) => {
                self.focus = if self.focus == Focus::Groups { Focus::Projects } else { Focus::Groups };
                if self.focus == Focus::Groups && self.glist.selected().is_none() && !self.groups.is_empty() {
                    self.glist.select(Some(0));
                }
            }
            (Focus::Projects, KeyCode::Down | KeyCode::Char('j')) => Self::move_list(&mut self.plist, n, 1),
            (Focus::Projects, KeyCode::Up | KeyCode::Char('k')) => Self::move_list(&mut self.plist, n, -1),
            (Focus::Groups, KeyCode::Down | KeyCode::Char('j')) => Self::move_list(&mut self.glist, self.groups.len(), 1),
            (Focus::Groups, KeyCode::Up | KeyCode::Char('k')) => Self::move_list(&mut self.glist, self.groups.len(), -1),
            (Focus::Projects, KeyCode::Enter) => {
                if let Some(p) = self.selected_project() {
                    if p["missing"] == true {
                        ctx.error("The project's folder is gone from disk — x removes it from the hub");
                    } else {
                        ctx.go(Route::Project(p["id"].as_str().unwrap_or_default().to_string()));
                    }
                }
            }
            (Focus::Groups, KeyCode::Enter) => {
                if let Some(g) = self.glist.selected().and_then(|i| self.groups.get(i)) {
                    ctx.go(Route::Group(g["id"].as_str().unwrap_or_default().to_string()));
                }
            }
            (_, KeyCode::Delete) => self.remove(ctx),
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn on_paste(&mut self, text: &str, ctx: &mut Ctx) {
        if let Some(job) = &self.job
            && job.running
        {
            ctx.send("job-input", Method::POST, format!("/jobs/{}/input", job.id), json!({ "data": text }));
        }
    }

    fn on_stream(&mut self, key: &str, msg: Value, ctx: &mut Ctx) {
        let Some(job) = &mut self.job else { return };
        if key != format!("job:{}", job.id) {
            return;
        }
        let was_running = job.running;
        job.on_stream(&msg);
        if msg["type"] == "__ended" {
            job.running = false;
        }
        if was_running && !job.running {
            if job.exit_code == Some(0) {
                ctx.info("cloned — the project is in the list");
            } else {
                ctx.error(format!("clone failed (exit {})", job.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())));
            }
            self.reload(ctx);
        }
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        let (what, arg) = key.split_once(':').unwrap_or((key, ""));
        match what {
            "projects" => {
                if let Some(v) = ctx.ok(result) {
                    let all = v.as_array().cloned().unwrap_or_default();
                    self.projects = all.iter().filter(|p| p["kind"] == "normal").cloned().collect();
                    ctx.shared.projects = all;
                    let n = self.shown().len();
                    if n == 0 {
                        self.plist.select(None);
                    } else {
                        self.plist.select(Some(self.plist.selected().unwrap_or(0).min(n - 1)));
                    }
                }
            }
            "groups" => {
                if let Some(v) = ctx.ok(result) {
                    self.groups = v.as_array().cloned().unwrap_or_default();
                    ctx.shared.groups = self.groups.clone();
                }
            }
            "job-input" => {
                let _ = result;
            }
            _ => self.on_answer(what, arg, result, ctx),
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = Theme::get();
        let [head, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(3)]).areas(area);
        let mut filters = vec![Span::styled(" Projects ", t.bold()), Span::styled(Self::storage(ctx), t.muted_style())];
        if !self.filter.is_empty() {
            filters.push(Span::styled(format!("   matching “{}”", self.filter), t.accent_style()));
        }
        if let Some(g) = &self.group_filter {
            filters.push(Span::styled(format!("   group: {}", self.group_name(g)), t.accent_style()));
        }
        if let Some(l) = &self.lang_filter {
            filters.push(Span::styled(format!("   language: {l}"), t.accent_style()));
        }
        filters
            .push(Span::styled(format!("   sorted by {}", if self.sort == Sort::Name { "name" } else { "last update" }), t.muted_style()));
        f.render_widget(Paragraph::new(Line::from(filters)), head);

        let [left, right] = Layout::horizontal([Constraint::Length((area.width / 4).clamp(22, 36)), Constraint::Min(30)]).areas(body);
        // groups
        let gitems: Vec<ListItem> = self
            .groups
            .iter()
            .map(|g| {
                let members: Vec<String> =
                    g["projectIds"].as_array().into_iter().flatten().filter_map(|id| id.as_str()).map(|id| self.project_name(id)).collect();
                let members = if members.is_empty() { "no repositories yet".to_string() } else { members.join(", ") };
                ListItem::new(vec![
                    Line::styled(format!("▣ {}", g["name"].as_str().unwrap_or_default()), t.accent_style()),
                    Line::styled(format!("  {members}"), t.muted_style()),
                ])
            })
            .collect();
        let gfocus = self.focus == Focus::Groups;
        if gitems.is_empty() {
            f.render_widget(
                Paragraph::new(vec![
                    Line::styled("Groups bring several repositories together for one agent session.", t.muted_style()),
                    Line::styled("G creates one.", t.muted_style()),
                ])
                .wrap(Wrap { trim: true })
                .block(block("groups — G new", gfocus)),
                left,
            );
        } else {
            let mut st = self.glist;
            if !gfocus {
                st.select(None);
            }
            f.render_stateful_widget(
                List::new(gitems).block(block("groups — G new · Enter open", gfocus)).highlight_style(t.selected()),
                left,
                &mut st,
            );
            if gfocus {
                self.glist = st;
            }
        }

        // projects
        let root = Self::storage(ctx);
        let shown = self.shown();
        let pitems: Vec<ListItem> = shown
            .iter()
            .map(|p| {
                let mut first =
                    vec![Span::styled(p["name"].as_str().unwrap_or_default().to_string(), t.accent_style().add_modifier(Modifier::BOLD))];
                for gid in p["groupIds"].as_array().into_iter().flatten().filter_map(|g| g.as_str()) {
                    first.push(Span::styled(format!("  ▣ {}", self.group_name(gid)), t.muted_style()));
                }
                let path = p["path"].as_str().unwrap_or_default();
                if p["missing"] == true {
                    return ListItem::new(vec![
                        Line::from(first),
                        Line::styled(format!("folder missing · {path} — x removes it from the hub"), Style::default().fg(t.warn)),
                        Line::raw(""),
                    ]);
                }
                let mut lines = vec![Line::from(first)];
                if let Some(d) = p["description"].as_str().filter(|d| !d.is_empty()) {
                    lines.push(Line::raw(d.to_string()));
                }
                let mut meta: Vec<Span> = vec![];
                if let Some(lang) = p["language"]["name"].as_str() {
                    let color = p["language"]["color"].as_str().and_then(hex_color).unwrap_or(t.muted);
                    meta.push(Span::styled("● ", Style::default().fg(color)));
                    meta.push(Span::styled(format!("{lang}   "), t.muted_style()));
                }
                match (p["gitProvider"].as_str(), p["repoUrl"].as_str()) {
                    (Some(_), Some(url)) => meta.push(Span::styled(format!("{}   ", slug_of(url)), t.muted_style())),
                    (Some(prov), None) => meta.push(Span::styled(format!("{prov}   "), t.muted_style())),
                    _ => meta.push(Span::styled("no remote   ", t.muted_style())),
                }
                let folder = path.strip_prefix(&format!("{root}/")).filter(|_| !root.is_empty()).unwrap_or(path);
                meta.push(Span::styled(format!("{folder}   "), t.muted_style()));
                meta.push(Span::styled(format!("updated {}", ago(p["updatedAt"].as_str().unwrap_or_default())), t.muted_style()));
                lines.push(Line::from(meta));
                lines.push(Line::raw(""));
                ListItem::new(lines)
            })
            .collect();
        let pfocus = self.focus == Focus::Projects;
        let title = format!("projects ({}) — Enter open · n new · x remove", shown.len());
        if pitems.is_empty() {
            let filtered = !self.filter.is_empty() || self.group_filter.is_some() || self.lang_filter.is_some();
            let text = if filtered { "No projects match." } else { "No projects yet — n clones, creates or imports one." };
            f.render_widget(Paragraph::new(Line::styled(text, t.muted_style())).block(block(title, pfocus)), right);
        } else {
            let mut st = self.plist;
            if !pfocus {
                st.select(None);
            }
            f.render_stateful_widget(List::new(pitems).block(block(title, pfocus)).highlight_style(t.selected()), right, &mut st);
            if pfocus {
                self.plist = st;
            }
        }

        if self.import.is_some() {
            self.draw_import(f, area);
        }
        if self.group.is_some() {
            self.draw_group(f, area);
        }
        if self.job.is_some() {
            self.draw_job(f, area);
        }
    }
}

impl Projects {
    /// Answers of modals and results of mutations.
    fn on_answer(&mut self, what: &str, arg: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        let Some(v) = ctx.ok(result) else {
            if what == "discover"
                && let Some(im) = &mut self.import
            {
                im.loading = false;
            }
            return;
        };
        let text = v.as_str().map(|s| s.to_string());
        match what {
            "filter" => {
                if let Some(f) = text {
                    self.filter = f;
                    self.plist.select(Some(0));
                }
            }
            "group-filter" => {
                if let Some(g) = text {
                    self.group_filter = (!g.is_empty()).then_some(g);
                    self.plist.select(Some(0));
                }
            }
            "lang-filter" => {
                if let Some(l) = text {
                    self.lang_filter = (!l.is_empty()).then_some(l);
                    self.plist.select(Some(0));
                }
            }
            "new-mode" => match text.as_deref() {
                Some("clone") => self.on_action("c", ctx),
                Some("empty") => self.on_action("e", ctx),
                Some("import") => self.on_action("i", ctx),
                _ => {}
            },
            "clone-url" => {
                if let Some(url) = text.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()) {
                    let name = derived_name(&url);
                    self.clone_url = url;
                    ctx.prompt("clone-name", format!("Folder name (in {}) — Enter keeps “{name}”", Self::storage(ctx)), name);
                }
            }
            "clone-name" => {
                if let Some(name) = text {
                    let (cols, rows) = self.term_size;
                    let mut body = json!({ "url": self.clone_url, "cols": cols, "rows": rows });
                    if !name.trim().is_empty() && name.trim() != derived_name(&self.clone_url) {
                        body["name"] = json!(name.trim());
                    }
                    ctx.send("cloning", Method::POST, "/projects/clone", body);
                }
            }
            "cloning" => {
                let job = JobTerm::new(&v);
                ctx.stream(format!("job:{}", job.id), job.stream_path());
                self.job = Some(job);
            }
            "new-repo" => {
                if let Some(name) = text.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
                    ctx.send("repo-created", Method::POST, "/projects/new-repo", json!({ "name": name }));
                }
            }
            "repo-created" => {
                self.reload(ctx);
                if let Some(id) = v["id"].as_str() {
                    ctx.go(Route::Project(id.to_string()));
                }
            }
            "discover" => {
                if let Some(im) = &mut self.import {
                    im.loading = false;
                    im.root = v["root"].as_str().unwrap_or(&im.root).to_string();
                    im.repos = v["repos"].as_array().cloned().unwrap_or_default();
                    im.list.select(if im.repos.is_empty() { None } else { Some(0) });
                }
            }
            "browse" => {
                if let Some(im) = &mut self.import {
                    im.browse = Some((v, ListState::default().with_selected(Some(0))));
                }
            }
            "browse-path" => {
                if let Some(p) = text.filter(|p| !p.trim().is_empty()) {
                    self.browse(Some(p.trim()), ctx);
                }
            }
            "import-filter" => {
                if let (Some(f), Some(im)) = (text, &mut self.import) {
                    im.filter = f;
                    im.list.select(Some(0));
                }
            }
            "imported" => {
                let list = v.as_array().cloned().unwrap_or_default();
                self.import = None;
                ctx.info(format!("imported {} projects", list.len()));
                self.reload(ctx);
                if list.len() == 1
                    && let Some(id) = list[0]["id"].as_str()
                {
                    ctx.go(Route::Project(id.to_string()));
                }
            }
            "group-name" => {
                if let Some(name) = text.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
                    self.group = Some(NewGroup {
                        name,
                        description: String::new(),
                        folder: String::new(),
                        picked: HashSet::new(),
                        list: ListState::default(),
                        filter: String::new(),
                    });
                    ctx.prompt("group-description", "Description (optional)", "");
                }
            }
            "group-description" => {
                if let (Some(d), Some(g)) = (text, &mut self.group) {
                    g.description = d;
                    let root = Self::storage(ctx);
                    ctx.prompt("group-folder", format!("Folder for the group's new projects (optional, default {root})"), "");
                } else {
                    self.group = None;
                }
            }
            "group-folder" => match (text, &mut self.group) {
                (Some(folder), Some(g)) => g.folder = folder,
                _ => self.group = None,
            },
            "member-filter" => {
                if let (Some(f), Some(g)) = (text, &mut self.group) {
                    g.filter = f;
                    g.list.select(Some(0));
                }
            }
            "group-created" => {
                self.group = None;
                self.reload(ctx);
                if let Some(id) = v["id"].as_str() {
                    ctx.go(Route::Group(id.to_string()));
                }
            }
            "remove-how" => match text.as_deref() {
                Some("keep") => ctx.send("removed", Method::DELETE, format!("/projects/{arg}"), Value::Null),
                Some("files") => {
                    let path = self.projects.iter().find(|p| p["id"] == arg).and_then(|p| p["path"].as_str()).unwrap_or_default();
                    let name = basename(path).to_string();
                    ctx.prompt(&format!("remove-confirm:{arg}"), format!("Type “{name}” to delete {path} for good"), "");
                }
                _ => {}
            },
            "remove-confirm" => {
                let Some(typed) = text else { return };
                let path = self.projects.iter().find(|p| p["id"] == arg).and_then(|p| p["path"].as_str()).unwrap_or_default();
                if typed.trim() != basename(path) {
                    ctx.error("The name doesn't match — nothing was removed");
                    return;
                }
                ctx.send("removed", Method::DELETE, format!("/projects/{arg}?files=1&confirm={}", enc(typed.trim())), Value::Null);
            }
            "remove" => {
                if v == Value::Bool(true) {
                    ctx.send("removed", Method::DELETE, format!("/projects/{arg}"), Value::Null);
                }
            }
            "removed" => {
                match v["deleted"].as_str() {
                    Some(d) => ctx.info(format!("removed; deleted {d}")),
                    None => ctx.info("removed from the hub"),
                }
                self.reload(ctx);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_urls() {
        assert_eq!(derived_name("git@github.com:acme/auth-service.git"), "auth-service");
        assert_eq!(derived_name("https://gitlab.com/acme/app/"), "app");
        assert_eq!(slug_of("git@github.com:acme/auth-service.git"), "acme/auth-service");
        assert_eq!(slug_of("https://github.com/acme/app"), "acme/app");
    }

    #[test]
    fn colors() {
        assert_eq!(hex_color("#3178c6"), Some(Color::Rgb(0x31, 0x78, 0xc6)));
        assert_eq!(hex_color("blue"), None);
    }
}
