//! The Repositories tab of a group (the web app's group workspace and the members part of
//! GroupSettings): member projects with branch and git status, open one, add / remove /
//! reorder members, a new empty repository in the group, and the context a group session
//! starts with.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent};
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};
use serde_json::{Value, json};

use crate::core::{Action, Ctx, Handled, Route, Screen};
use crate::ui::{Theme, block};

/// `owner/name` of a remote URL (what the web app shows next to a member).
pub fn short_remote(url: &str) -> String {
    let url = url.trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let parts: Vec<&str> = url.split(['/', ':']).filter(|s| !s.is_empty()).collect();
    match parts.as_slice() {
        [.., owner, name] => format!("{owner}/{name}"),
        _ => url.to_string(),
    }
}

/// "main ↑1 ↓2 · 3 changed" from a git status.
pub fn status_text(st: &Value) -> String {
    if st["isRepo"] == false {
        return "not a git repository".into();
    }
    let mut s = st["branch"].as_str().unwrap_or(if st["detached"] == true { "detached" } else { "—" }).to_string();
    if let Some(n) = st["ahead"].as_u64().filter(|n| *n > 0) {
        s.push_str(&format!(" ↑{n}"));
    }
    if let Some(n) = st["behind"].as_u64().filter(|n| *n > 0) {
        s.push_str(&format!(" ↓{n}"));
    }
    match st["files"].as_array().map(Vec::len).unwrap_or(0) {
        0 => s.push_str(" · clean"),
        n => s.push_str(&format!(" · {n} changed")),
    }
    if let Some(state) = st["state"].as_str() {
        s.push_str(&format!(" · {state} in progress"));
    }
    s
}

pub struct ReposTab {
    id: String,
    group: Value,
    git: HashMap<String, Value>,
    list: ListState,
    context: Option<String>,
    context_scroll: u16,
}

impl ReposTab {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            group: Value::Null,
            git: HashMap::new(),
            list: ListState::default().with_selected(Some(0)),
            context: None,
            context_scroll: 0,
        }
    }

    fn projects(&self) -> Vec<Value> {
        self.group["projects"].as_array().cloned().unwrap_or_default()
    }

    fn ids(&self) -> Vec<String> {
        self.group["projectIds"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect()
    }

    fn selected(&self) -> Option<Value> {
        self.list.selected().and_then(|i| self.projects().get(i).cloned())
    }

    fn reload(&self, ctx: &mut Ctx) {
        ctx.get("group", format!("/groups/{}", self.id));
    }

    fn set_members(&self, ids: Vec<String>, ctx: &mut Ctx) {
        ctx.send("group", Method::PATCH, format!("/groups/{}", self.id), json!({ "projectIds": ids }));
    }

    fn move_selected(&mut self, delta: isize, ctx: &mut Ctx) {
        let Some(i) = self.list.selected() else { return };
        let mut ids = self.ids();
        let j = i as isize + delta;
        if j < 0 || j as usize >= ids.len() {
            return;
        }
        ids.swap(i, j as usize);
        self.list.select(Some(j as usize));
        self.set_members(ids, ctx);
    }
}

impl Screen for ReposTab {
    fn title(&self) -> String {
        "Repositories".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        self.reload(ctx);
    }

    fn actions(&self) -> Vec<Action> {
        vec![
            Action::new("o", "Open the selected project"),
            Action::new("a", "Add a project to the group"),
            Action::new("x", "Remove the selected project from the group"),
            Action::new("K", "Move the selected up"),
            Action::new("J", "Move the selected down"),
            Action::new("n", "New empty repository in the group"),
            Action::new("c", if self.context.is_some() { "Refresh what the agent gets" } else { "Show what the agent gets" }),
            Action::new("C", "Hide the agent context"),
            Action::new("r", "Reload"),
        ]
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        match key {
            "o" => {
                if let Some(p) = self.selected() {
                    ctx.go(Route::Project(p["id"].as_str().unwrap_or_default().to_string()));
                }
            }
            "a" => {
                let ids = self.ids();
                let items: Vec<(String, String)> = ctx
                    .shared
                    .projects
                    .iter()
                    .filter(|p| p["kind"] == "normal" && !ids.iter().any(|i| p["id"] == i.as_str()))
                    .map(|p| {
                        (
                            p["id"].as_str().unwrap_or_default().to_string(),
                            format!("{} — {}", p["name"].as_str().unwrap_or_default(), p["path"].as_str().unwrap_or_default()),
                        )
                    })
                    .collect();
                if items.is_empty() {
                    ctx.info("every project is already in the group — n creates a new repository");
                } else {
                    ctx.pick("add", "Add a project to the group", items);
                }
            }
            "x" => {
                if let Some(p) = self.selected() {
                    ctx.confirm(
                        &format!("remove:{}", p["id"].as_str().unwrap_or_default()),
                        "Remove from the group",
                        format!("Remove {} from the group? The project and its files stay.", p["name"].as_str().unwrap_or_default()),
                        false,
                    );
                }
            }
            "K" => self.move_selected(-1, ctx),
            "J" => self.move_selected(1, ctx),
            "n" => ctx.prompt("new-repo", "New repository in the group — name", ""),
            "c" => ctx.get("context", format!("/groups/{}/context", self.id)),
            "C" => self.context = None,
            "r" => self.reload(ctx),
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        let n = self.projects().len();
        match k.code {
            KeyCode::Down | KeyCode::Char('j') if n > 0 => self.list.select(Some((self.list.selected().unwrap_or(0) + 1).min(n - 1))),
            KeyCode::Up | KeyCode::Char('k') => self.list.select(Some(self.list.selected().unwrap_or(0).saturating_sub(1))),
            KeyCode::Enter => self.on_action("o", ctx),
            KeyCode::PageDown if self.context.is_some() => self.context_scroll = self.context_scroll.saturating_add(10),
            KeyCode::PageUp if self.context.is_some() => self.context_scroll = self.context_scroll.saturating_sub(10),
            _ if crate::screens::kit::list_motion(&mut self.list, &k, n) => {}
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        if let Some(pid) = key.strip_prefix("git:") {
            // a member without git (or an unreachable folder) just shows no status
            if let Ok(st) = result {
                self.git.insert(pid.to_string(), st);
            }
            return;
        }
        let Some(v) = ctx.ok(result) else { return };
        match key {
            "group" => {
                self.group = v;
                for p in self.projects() {
                    let pid = p["id"].as_str().unwrap_or_default().to_string();
                    ctx.get(format!("git:{pid}"), format!("/projects/{pid}/git/status"));
                }
                let n = self.projects().len();
                if n > 0 && self.list.selected().is_none_or(|i| i >= n) {
                    self.list.select(Some(n - 1));
                }
                if let Some(g) = ctx.shared.groups.iter_mut().find(|g| g["id"] == self.id.as_str()) {
                    for k in ["name", "projectIds", "folder", "description", "instructions"] {
                        g[k] = self.group[k].clone();
                    }
                }
                if self.context.is_some() {
                    ctx.get("context", format!("/groups/{}/context", self.id));
                }
            }
            "add" => {
                if let Some(pid) = v.as_str() {
                    let mut ids = self.ids();
                    ids.push(pid.to_string());
                    self.set_members(ids, ctx);
                }
            }
            k if k.starts_with("remove:") && v == Value::Bool(true) => {
                let pid = &k[7..];
                let ids = self.ids().into_iter().filter(|i| i != pid).collect();
                self.set_members(ids, ctx);
            }
            "new-repo" => {
                if let Some(name) = v.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    ctx.send("created", Method::POST, "/projects/new-repo", json!({ "name": name, "groupId": self.id }));
                }
            }
            "created" => {
                ctx.info(format!("created {}", v["path"].as_str().unwrap_or_default()));
                ctx.shared.projects.push(v);
                self.reload(ctx);
            }
            "context" => {
                self.context = v["context"].as_str().map(String::from);
                self.context_scroll = 0;
            }
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        let t = Theme::get();
        let (list_area, ctx_area) = if self.context.is_some() {
            let [a, b] = Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(area);
            (a, Some(b))
        } else {
            (area, None)
        };
        let [list_area, path_area] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(list_area);
        let projects = self.projects();
        let title = format!("repositories ({}) — Enter open · a add · x remove · K/J reorder · c agent context", projects.len());
        if projects.is_empty() {
            let msg = if self.group.is_null() {
                "loading…"
            } else {
                "No repositories in the group yet — a adds an existing project, n creates a new repository."
            };
            f.render_widget(
                Paragraph::new(Span::styled(msg, t.muted_style())).wrap(Wrap { trim: true }).block(block(title, true)),
                list_area,
            );
        } else {
            let items: Vec<ListItem> = projects
                .iter()
                .map(|p| {
                    let pid = p["id"].as_str().unwrap_or_default();
                    let (status, color) = match self.git.get(pid) {
                        Some(st) => {
                            let dirty = st["files"].as_array().is_some_and(|f| !f.is_empty());
                            (status_text(st), if dirty { t.warn } else { t.ok })
                        }
                        None => ("…".to_string(), t.muted),
                    };
                    let remote = p["repoUrl"].as_str().map(short_remote).unwrap_or_else(|| "no remote".into());
                    ListItem::new(Line::from(vec![
                        Span::styled(" ⎇ ", t.accent_style()),
                        Span::styled(format!("{:<24}", p["name"].as_str().unwrap_or_default()), t.bold()),
                        Span::styled(format!(" {status:<32}"), Style::default().fg(color)),
                        Span::styled(format!(" {remote}"), t.muted_style()),
                        Span::styled(if p["missing"] == true { "  (folder missing)" } else { "" }, Style::default().fg(t.danger)),
                    ]))
                })
                .collect();
            f.render_stateful_widget(List::new(items).block(block(title, true)).highlight_style(t.selected()), list_area, &mut self.list);
        }
        let mut foot =
            vec![Span::styled(format!(" workspace: {}", self.group["workspace"]["path"].as_str().unwrap_or_default()), t.muted_style())];
        if self.group["workspace"]["status"].as_str().is_some_and(|s| s != "ready") {
            foot.push(Span::styled(
                format!("  · {}", self.group["workspace"]["statusDetail"].as_str().unwrap_or("not ready")),
                Style::default().fg(t.warn),
            ));
        }
        f.render_widget(Paragraph::new(Line::from(foot)), path_area);
        if let (Some(area), Some(text)) = (ctx_area, &self.context) {
            f.render_widget(
                Paragraph::new(text.clone())
                    .wrap(Wrap { trim: false })
                    .scroll((self.context_scroll, 0))
                    .block(block("what the agent gets — PgUp/PgDn scroll · c refresh · C hide", false)),
                area,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remotes() {
        assert_eq!(short_remote("git@github.com:acme/api.git"), "acme/api");
        assert_eq!(short_remote("https://gitlab.com/acme/web/"), "acme/web");
    }

    #[test]
    fn statuses() {
        let st = json!({ "isRepo": true, "branch": "main", "ahead": 1, "behind": 0, "files": [{}], "state": null });
        assert_eq!(status_text(&st), "main ↑1 · 1 changed");
        assert_eq!(status_text(&json!({ "isRepo": false })), "not a git repository");
    }
}
