//! Harness → Skills (the web app's Skills page): skills × agents, read from each agent's
//! skills folders. The first column is the skill (Enter or `v` shows its SKILL.md), the
//! others are the agents: Enter copies the skill into that agent's folder or moves the
//! agent's copy into the hub trash. `n` writes a new skill, `i` imports skills from a git
//! repository, `x` removes a skill from every agent. Project and plugin skills are read-only.

use crossterm::event::{KeyCode, KeyEvent};
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph, Wrap};
use serde_json::{Value, json};

use super::harness_ext::tilde;
use super::kit::{MultiPick, Nav, TextView, click_items, click_select, clicked_row, s};
use super::session::enc;
use crate::core::{Action, Ctx, Handled, Hit, Screen};
use crate::ui::theme::icon;
use crate::ui::{Theme, block};

/// What the tab is, in plain words (the line under the tab bar).
pub const SKILLS_HELP: &str = "Packaged instructions (a folder with SKILL.md). Only the name and description sit in the agent's context; the agent loads a skill by itself when it fits the task. Viewing one here does not load it anywhere.";

/// A skill being written through prompts.
#[derive(Default, Clone)]
struct NewSkill {
    name: String,
    description: String,
    body: String,
}

#[derive(Default)]
pub struct SkillsTab {
    data: Value,
    error: Option<String>,
    nav: Nav,
    /// The column under the cursor: 0 — the skill itself, 1… — the agents.
    col: usize,
    project: Option<(String, String)>,
    draft: NewSkill,
    /// The last POST that answered 409 (sent again with `replace: true` when confirmed).
    retry: Option<(String, Value)>,
    /// The import being set up: its preview, then the skills and agents chosen.
    preview: Value,
    pick_skills: MultiPick,
    pick_agents: MultiPick,
    /// SKILL.md open over the matrix.
    viewer: Option<TextView>,
}

const NAME_W: usize = 28;
const CELL_W: usize = 13;

impl SkillsTab {
    pub fn new() -> Self {
        Self::default()
    }

    fn query(&self) -> String {
        self.project.as_ref().map(|(id, _)| format!("?project={}", enc(id))).unwrap_or_default()
    }

    fn load(&self, ctx: &Ctx) {
        ctx.get("skills", format!("/harness/skills{}", self.query()));
    }

    fn skills(&self) -> Vec<Value> {
        self.data["skills"].as_array().cloned().unwrap_or_default()
    }

    fn agents(&self) -> Vec<Value> {
        self.data["agents"].as_array().cloned().unwrap_or_default()
    }

    fn selected(&self) -> Option<Value> {
        self.nav.selected().and_then(|i| self.skills().get(i).cloned())
    }

    fn move_col(&mut self, d: isize) {
        let n = self.agents().len() + 1;
        self.col = (self.col as isize + d).rem_euclid(n as isize) as usize;
    }

    /// The agents the hub can write skills for, as picker items (the others marked).
    fn agent_items(&self) -> Vec<(String, String)> {
        self.agents()
            .iter()
            .map(|a| {
                let (kind, label) = (s(&a["kind"], ""), s(&a["label"], ""));
                if a["supported"] == true {
                    (kind.to_string(), format!("{label}  {}", tilde(s(&a["dir"], ""))))
                } else {
                    (format!("!{kind}"), format!("{label} — the hub doesn't know its skills folder"))
                }
            })
            .collect()
    }

    fn view(&mut self, ctx: &mut Ctx) {
        let Some(skill) = self.selected() else { return };
        let name = s(&skill["name"], "").to_string();
        let agent = (self.col > 0).then(|| self.agents().get(self.col - 1).map(|a| s(&a["kind"], "").to_string())).flatten();
        let mut path = format!("/harness/skills/{}", enc(&name));
        let mut q = vec![];
        if let Some(a) = agent.filter(|a| skill["agents"][a.as_str()].is_string()) {
            q.push(format!("agent={}", enc(&a)));
        }
        if let Some((id, _)) = &self.project {
            q.push(format!("project={}", enc(id)));
        }
        if !q.is_empty() {
            path.push_str(&format!("?{}", q.join("&")));
        }
        self.viewer = Some(TextView::new(format!("SKILL.md · {name} (read-only)"), false));
        ctx.get("view", path);
    }

    /// Enter on a cell: copy the skill to the agent, or move the agent's copy to the trash.
    fn toggle(&mut self, ctx: &mut Ctx) {
        if self.col == 0 {
            return self.view(ctx);
        }
        let (Some(skill), Some(agent)) = (self.selected(), self.agents().get(self.col - 1).cloned()) else { return };
        let (name, kind) = (s(&skill["name"], ""), s(&agent["kind"], ""));
        let label = s(&agent["label"], kind);
        match skill["agents"][kind].as_str() {
            Some(scope @ ("project" | "plugin")) => {
                let from = skill["entries"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|e| e["agent"] == kind && e["scope"] == scope)
                    .map(|e| tilde(s(&e["path"], "")))
                    .unwrap_or_default();
                ctx.info(format!(
                    "{name} comes from {} ({from}) — read-only here",
                    if scope == "plugin" { "a plugin" } else { "the project" }
                ));
            }
            Some(_) => {
                ctx.info(format!("moving {name} of {label} to the hub trash…"));
                ctx.send(
                    "changed",
                    Method::DELETE,
                    format!("/harness/skills/{}/agents/{}{}", enc(name), enc(kind), self.query()),
                    Value::Null,
                );
            }
            None => {
                if agent["supported"] != true {
                    return ctx.error(format!("{label}: the hub doesn't know where this agent keeps skills"));
                }
                ctx.info(format!("copying {name} to {label}…"));
                ctx.send(
                    "changed",
                    Method::PUT,
                    format!("/harness/skills/{}/agents/{}{}", enc(name), enc(kind), self.query()),
                    Value::Null,
                );
            }
        }
    }

    /// A POST that may answer 409 (an existing different skill): kept to retry with `replace`.
    fn post(&mut self, key: &str, path: String, body: Value, ctx: &mut Ctx) {
        self.retry = Some((path.clone(), body.clone()));
        ctx.send(key, Method::POST, path, body);
    }

    fn cell(&self, skill: &Value, agent: &Value) -> (String, Style) {
        let t = Theme::get();
        match skill["agents"][s(&agent["kind"], "")].as_str() {
            Some("project") => (format!("{} project", icon("\u{f07b}", "+")), t.accent_style()),
            Some("plugin") => (format!("{} plugin", icon("\u{f12e}", "+")), t.accent_style()),
            Some(_) => (format!("{} on", icon("\u{f205}", "[x]")), Style::default().fg(t.ok)),
            None if agent["supported"] != true => ("  n/a".into(), t.muted_style()),
            None => (format!("{} ·", icon("\u{f204}", "[ ]")), t.muted_style()),
        }
    }

    fn details(&self, skill: &Value) -> Vec<Line<'static>> {
        let t = Theme::get();
        let mut l = vec![Line::raw(format!(" {}", s(&skill["description"], "(no description)")))];
        for e in skill["entries"].as_array().into_iter().flatten() {
            l.push(Line::styled(
                format!(" {:<12} {:<8} {}", s(&e["agent"], ""), s(&e["scope"], ""), tilde(s(&e["path"], ""))),
                t.muted_style(),
            ));
        }
        l
    }
}

impl Screen for SkillsTab {
    fn title(&self) -> String {
        "Skills".into()
    }

    fn help(&self) -> Option<String> {
        Some("instructions the agent loads by itself when they fit the task".into())
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        self.load(ctx);
    }

    fn actions(&self) -> Vec<Action> {
        if self.viewer.is_some() {
            return vec![Action::new("Esc", "Close SKILL.md")];
        }
        let mut a = vec![
            Action::new("n", "New skill"),
            Action::new("i", "Import skills from a git repository"),
            Action::new("r", "Reload"),
            Action::new("P", "Show a project's skills too"),
            Action::new("[", "Previous column"),
            Action::new("]", "Next column"),
        ];
        if self.selected().is_some() {
            a.insert(0, Action::new("v", "View SKILL.md"));
            a.insert(3, Action::new("x", "Remove the skill from every agent (to the hub trash)"));
        }
        a
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        match key {
            "v" => self.view(ctx),
            "n" => {
                self.draft = NewSkill::default();
                ctx.prompt("new-name", "Skill name (letters, digits, - and _)", "");
            }
            "i" => ctx.prompt("imp-url", "Git repository: https URL or owner/repo (e.g. anthropics/skills)", ""),
            "r" => self.load(ctx),
            "x" => {
                if let Some(skill) = self.selected() {
                    let name = s(&skill["name"], "").to_string();
                    ctx.confirm(
                        &format!("delete:{name}"),
                        "Remove skill",
                        format!("Move {name} of every agent into the hub trash? Project and plugin copies stay."),
                        true,
                    );
                }
            }
            "P" => {
                let mut items = vec![("".to_string(), "No project — user folders only".to_string())];
                items.extend(
                    ctx.shared
                        .projects
                        .iter()
                        .filter(|p| p["kind"] == "normal")
                        .map(|p| (format!("{}\u{1f}{}", s(&p["id"], ""), s(&p["name"], "")), s(&p["name"], "").to_string())),
                );
                ctx.pick("project", "Show a project's skills too", items);
            }
            "[" => self.move_col(-1),
            "]" => self.move_col(1),
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        if let Some(v) = &mut self.viewer {
            if v.key(&k) == Some(false) {
                self.viewer = None;
            }
            return Handled::Yes;
        }
        let n = self.skills().len();
        if self.nav.key(&k, n) {
            return Handled::Yes;
        }
        match k.code {
            KeyCode::Left | KeyCode::BackTab => self.move_col(-1),
            KeyCode::Right | KeyCode::Tab => self.move_col(1),
            KeyCode::Enter => self.toggle(ctx),
            KeyCode::Delete => self.on_action("x", ctx),
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn on_click(&mut self, id: &str, ctx: &mut Ctx) {
        if let Some(rest) = id.strip_prefix("cell:")
            && let Some((r, c)) = rest.split_once(':')
            && let (Ok(r), Ok(c)) = (r.parse::<usize>(), c.parse::<usize>())
        {
            let again = self.nav.selected() == Some(r) && self.col == c;
            self.nav.state.select(Some(r));
            self.col = c;
            if again {
                self.toggle(ctx);
            }
        } else if let Some(c) = id.strip_prefix("col:").and_then(|c| c.parse().ok()) {
            self.col = c;
        } else if let Some(i) = clicked_row(id, "rows") {
            click_select(&mut self.nav.state, i);
        }
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        match key {
            "skills" => match result {
                Ok(v) => {
                    self.data = v;
                    self.error = None;
                }
                Err(e) => self.error = Some(e.to_string()),
            },
            "changed" => {
                if let Some(v) = ctx.ok(result) {
                    self.data = v;
                    ctx.info("saved to the agents' skills folders");
                }
            }
            "created" | "imported" => match result {
                Ok(v) => {
                    self.data = v;
                    self.retry = None;
                    ctx.info(if key == "created" { "skill written" } else { "skills imported" });
                }
                Err(e) if e.to_string().starts_with("409:") => {
                    let msg = e.to_string().trim_start_matches("409:").trim().to_string();
                    ctx.confirm(&format!("replace:{key}"), "Replace skill", format!("{msg}\n\nReplace it?"), true);
                }
                Err(e) => ctx.error(e),
            },
            k if k.starts_with("replace:") => {
                if matches!(result, Ok(Value::Bool(true)))
                    && let Some((path, mut body)) = self.retry.take()
                {
                    body["replace"] = json!(true);
                    ctx.send(&k["replace:".len()..], Method::POST, path, body);
                }
            }
            "view" => match result {
                Ok(v) => {
                    let mut text = s(&v["content"], "").to_string();
                    let files: Vec<String> = v["files"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|f| format!("  {} ({} B)", s(&f["path"], ""), f["size"].as_u64().unwrap_or(0)))
                        .collect();
                    text.push_str(&format!(
                        "\n\n──── {} · {} · {}\n{}",
                        s(&v["agent"], ""),
                        s(&v["scope"], ""),
                        tilde(s(&v["path"], "")),
                        files.join("\n")
                    ));
                    if let Some(view) = &mut self.viewer {
                        view.set(text);
                    }
                }
                Err(e) => {
                    self.viewer = None;
                    ctx.error(e);
                }
            },
            "project" => {
                if let Ok(Value::String(id)) = result {
                    self.project = id.split_once('\u{1f}').map(|(i, n)| (i.to_string(), n.to_string()));
                    self.load(ctx);
                }
            }
            k if k.starts_with("delete:") => {
                if matches!(result, Ok(Value::Bool(true))) {
                    let name = &k["delete:".len()..];
                    ctx.send("changed", Method::DELETE, format!("/harness/skills/{}{}", enc(name), self.query()), Value::Null);
                }
            }
            // ---- a new skill ----
            "new-name" => {
                let Ok(Value::String(name)) = result else { return };
                self.draft.name = name.trim().to_string();
                if !self.draft.name.is_empty() {
                    ctx.prompt("new-desc", "Description: what it does and when the agent should use it", "");
                }
            }
            "new-desc" => {
                let Ok(Value::String(d)) = result else { return };
                self.draft.description = d.trim().to_string();
                ctx.prompt_multiline("new-body", "Instructions (markdown; Alt+Enter for a new line, Enter saves)", "");
            }
            "new-body" => {
                let Ok(Value::String(body)) = result else { return };
                self.draft.body = body;
                let items = self.agent_items();
                let chosen = items.iter().filter(|(k, _)| !k.starts_with('!')).map(|(k, _)| k.clone()).collect();
                self.pick_agents = MultiPick::new("new-agents", format!("Write {} for", self.draft.name), items, chosen);
                self.pick_agents.open(ctx, 0);
            }
            "new-agents" => {
                if self.pick_agents.answer(&result, ctx) != Some(true) {
                    return;
                }
                let d = std::mem::take(&mut self.draft);
                let body = json!({ "name": d.name, "description": d.description, "body": d.body, "agents": self.pick_agents.chosen });
                ctx.info(format!("writing {}…", d.name));
                self.post("created", format!("/harness/skills{}", self.query()), body, ctx);
            }
            // ---- import from git ----
            "imp-url" => {
                let Ok(Value::String(url)) = result else { return };
                let url = url.trim().to_string();
                if url.is_empty() {
                    return;
                }
                ctx.info(format!("cloning {url}…"));
                ctx.send("imp-preview", Method::POST, "/harness/skills/import/preview", json!({ "url": url }));
            }
            "imp-preview" => {
                let Some(v) = ctx.ok(result) else { return };
                let items: Vec<(String, String)> = v["skills"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|sk| {
                        let desc: String = s(&sk["description"], "").chars().take(70).collect();
                        (s(&sk["name"], "").to_string(), format!("{} — {desc}", s(&sk["name"], "")))
                    })
                    .collect();
                ctx.info(format!("{} skills in {}", items.len(), s(&v["url"], "")));
                self.pick_skills = MultiPick::new("imp-skills", "Skills to import", items, vec![]);
                self.preview = v;
                self.pick_skills.open(ctx, 1);
            }
            "imp-skills" => {
                if self.pick_skills.answer(&result, ctx) != Some(true) {
                    return;
                }
                if self.pick_skills.chosen.is_empty() {
                    return ctx.error("no skill chosen");
                }
                let items = self.agent_items();
                let chosen = items.iter().filter(|(k, _)| !k.starts_with('!')).map(|(k, _)| k.clone()).collect();
                self.pick_agents = MultiPick::new("imp-agents", "Import for", items, chosen);
                self.pick_agents.open(ctx, 0);
            }
            "imp-agents" => {
                if self.pick_agents.answer(&result, ctx) != Some(true) {
                    return;
                }
                let body = json!({ "id": self.preview["id"], "skills": self.pick_skills.chosen, "agents": self.pick_agents.chosen });
                ctx.info("importing…");
                self.post("imported", format!("/harness/skills/import{}", self.query()), body, ctx);
            }
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = Theme::get();
        let skills = self.skills();
        let agents = self.agents();
        let [hint, head, body, detail] =
            Layout::vertical([Constraint::Length(2), Constraint::Length(1), Constraint::Min(4), Constraint::Length(8)]).areas(area);
        let project = self.project.as_ref().map(|(_, n)| format!("   project: {n}")).unwrap_or_default();
        f.render_widget(Paragraph::new(Line::styled(format!(" {SKILLS_HELP}{project}"), t.muted_style())).wrap(Wrap { trim: false }), hint);
        if let Some(v) = &mut self.viewer {
            let [_, rest] = Layout::vertical([Constraint::Length(2), Constraint::Min(3)]).areas(area);
            v.draw(f, rest, "Esc closes");
            return;
        }
        if self.data.is_null() {
            let msg = self.error.clone().unwrap_or_else(|| "Loading…".into());
            f.render_widget(Paragraph::new(msg).wrap(Wrap { trim: false }).block(block("Skills", true)), body);
            return;
        }
        self.col = self.col.min(agents.len());
        let name_style = if self.col == 0 { t.key_style() } else { t.dim_style() };
        let mut spans = vec![Span::raw(" "), Span::styled(format!("{:<NAME_W$}", "skill"), name_style)];
        ctx.click(Rect { x: head.x + 1, y: head.y, width: NAME_W as u16, height: 1 }, Hit::Click("col:0".into()));
        let mut x = head.x + 1 + NAME_W as u16;
        for (i, a) in agents.iter().enumerate() {
            let label: String = s(&a["label"], "").chars().take(CELL_W - 1).collect();
            let style = if i + 1 == self.col { t.key_style() } else { t.dim_style() };
            spans.push(Span::styled(format!("{label:<w$}", w = CELL_W - 1), style));
            spans.push(Span::raw(" "));
            ctx.click(
                Rect { x, y: head.y, width: (CELL_W as u16).min((head.x + head.width).saturating_sub(x)), height: 1 },
                Hit::Click(format!("col:{}", i + 1)),
            );
            x += CELL_W as u16;
        }
        f.render_widget(Paragraph::new(Line::from(spans)), head);

        self.nav.clamp(skills.len());
        let selected = self.nav.selected();
        let items: Vec<ListItem> = skills
            .iter()
            .enumerate()
            .map(|(r, sk)| {
                let name: String = s(&sk["name"], "").chars().take(NAME_W - 1).collect();
                let mut style = t.bold();
                if selected == Some(r) && self.col == 0 {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                let mut spans = vec![Span::styled(format!("{name:<NAME_W$}"), style)];
                for (c, a) in agents.iter().enumerate() {
                    let (text, mut style) = self.cell(sk, a);
                    if selected == Some(r) && c + 1 == self.col {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                    spans.push(Span::styled(format!("{text:<w$}", w = CELL_W - 1), style));
                    spans.push(Span::raw(" "));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();
        let title = "Skills — Enter toggle / view · [ ] column · v view · n new · i import · x remove · P project";
        let b = block(title, true);
        let inner = b.inner(body);
        if skills.is_empty() {
            f.render_widget(
                Paragraph::new(Line::styled(" No skills yet — n writes one, i imports some from a git repository.", t.muted_style()))
                    .block(b),
                body,
            );
        } else {
            f.render_stateful_widget(
                List::new(items).block(b).highlight_style(Style::default().bg(t.selection)),
                body,
                &mut self.nav.state,
            );
            click_items(ctx, "rows", body, true, &self.nav.state, &vec![1; skills.len()]);
            for row in 0..(inner.height as usize).min(skills.len().saturating_sub(self.nav.state.offset())) {
                let r = self.nav.state.offset() + row;
                for c in 0..=agents.len() {
                    let (cx, w) = if c == 0 {
                        (inner.x, NAME_W as u16)
                    } else {
                        (inner.x + NAME_W as u16 + ((c - 1) * CELL_W) as u16, CELL_W as u16)
                    };
                    if cx >= inner.x + inner.width {
                        break;
                    }
                    let w = w.min(inner.x + inner.width - cx);
                    ctx.click(Rect { x: cx, y: inner.y + row as u16, width: w, height: 1 }, Hit::Click(format!("cell:{r}:{c}")));
                }
            }
        }

        let mut lines = selected.and_then(|i| skills.get(i)).map(|sk| self.details(sk)).unwrap_or_default();
        if let Some(a) = self.col.checked_sub(1).and_then(|c| agents.get(c)) {
            lines.push(Line::styled(format!(" {} skills folder: {}", s(&a["label"], ""), tilde(s(&a["dir"], "—"))), t.muted_style()));
        }
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block("details", false)), detail);
    }
}
