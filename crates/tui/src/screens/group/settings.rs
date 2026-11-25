//! The Settings tab of a group (the web app's GroupSettings): name, description, shared AI
//! instructions, the storage folder for new projects, the members and deleting the group.

use crossterm::event::{KeyCode, KeyEvent};
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use serde_json::{Value, json};

use crate::core::{Action, Ctx, Handled, Route, Screen};
use crate::screens::project::form::{Form, Row, value};
use crate::ui::Theme;

#[derive(Clone, PartialEq)]
enum Act {
    Name,
    Description,
    Instructions,
    Folder,
    Project(String),
    Delete,
}

pub struct SettingsTab {
    id: String,
    group: Value,
    form: Form<Act>,
}

impl SettingsTab {
    pub fn new(id: &str) -> Self {
        Self { id: id.to_string(), group: Value::Null, form: Form::default() }
    }

    fn path(&self) -> String {
        format!("/groups/{}", self.id)
    }

    fn rebuild(&mut self, ctx: &Ctx) {
        let t = Theme::get();
        let g = &self.group;
        let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
        let field = |label: &str, value: Line<'static>, act: Option<Act>| Row::Field { label: label.into(), value, act };
        let root = ctx.shared.settings["projectsRoot"].as_str().unwrap_or("the projects folder").to_string();
        let mut rows = vec![
            Row::Head("Group".into()),
            field("Name", value(&s(&g["name"]), "—"), Some(Act::Name)),
            field("Description", value(&s(&g["description"]), "What this group is"), Some(Act::Description)),
            field("AI instructions", value(&s(&g["instructions"]), "none"), Some(Act::Instructions)),
            Row::Text("Added to every group session; repository instructions win when they conflict.".into()),
            field("Storage folder", value(&s(&g["folder"]), &format!("default ({root})")), Some(Act::Folder)),
            Row::Text(format!("New projects created from this group go here. Empty — the projects folder ({root}).")),
            Row::Head("Repositories".into()),
        ];
        let projects = g["projects"].as_array().cloned().unwrap_or_default();
        if projects.is_empty() {
            rows.push(Row::Text("No repositories yet — add them in the Repositories tab.".into()));
        }
        for p in &projects {
            let remote = p["repoUrl"].as_str().map(super::repos::short_remote).unwrap_or_else(|| "no remote".into());
            rows.push(field(&s(&p["name"]), Line::styled(remote, t.muted_style()), Some(Act::Project(s(&p["id"])))));
        }
        rows.push(Row::Text(format!(
            "Workspace: {}. Add, remove and reorder members in the Repositories tab.",
            g["workspace"]["path"].as_str().unwrap_or_default()
        )));
        if g.is_object() {
            rows.push(Row::Head("Danger zone".into()));
            rows.push(field(
                "Delete group",
                Line::styled("deletes the group and its sessions; projects and their files stay", Style::default().fg(t.danger)),
                Some(Act::Delete),
            ));
        }
        self.form.set(rows);
    }

    fn run(&mut self, act: Act, ctx: &mut Ctx) {
        let g = &self.group;
        let s = |k: &str| g[k].as_str().unwrap_or_default().to_string();
        match act {
            Act::Name => ctx.prompt("name", "Group name", s("name")),
            Act::Description => ctx.prompt_multiline("description", "Description (Enter saves, Alt+Enter newline)", s("description")),
            Act::Instructions => ctx.prompt_multiline(
                "instructions",
                "Shared instructions for AI agents (Enter saves, Alt+Enter newline)",
                s("instructions"),
            ),
            Act::Folder => ctx.prompt("folder", "Storage folder for new projects (empty: the projects folder)", s("folder")),
            Act::Project(id) => ctx.go(Route::Project(id)),
            Act::Delete => ctx.confirm(
                "delete",
                "Delete group?",
                format!("Delete the group “{}” and its sessions? Projects and their files stay.", s("name")),
                true,
            ),
        }
    }
}

impl Screen for SettingsTab {
    fn title(&self) -> String {
        "Settings".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        ctx.get("group", self.path());
    }

    fn actions(&self) -> Vec<Action> {
        vec![
            Action::new("n", "Rename"),
            Action::new("d", "Description"),
            Action::new("I", "Shared AI instructions"),
            Action::new("f", "Storage folder"),
            Action::new("D", "Delete the group"),
            Action::new("r", "Reload"),
        ]
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        let act = match key {
            "n" => Act::Name,
            "d" => Act::Description,
            "I" => Act::Instructions,
            "f" => Act::Folder,
            "D" => Act::Delete,
            "r" => return self.on_show(ctx),
            _ => return,
        };
        self.run(act, ctx);
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => self.form.step(1),
            KeyCode::Up | KeyCode::Char('k') => self.form.step(-1),
            KeyCode::PageDown => self.form.step(10),
            KeyCode::PageUp => self.form.step(-10),
            KeyCode::Enter => {
                if let Some(a) = self.form.current() {
                    self.run(a, ctx);
                }
            }
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        let Some(v) = ctx.ok(result) else { return };
        match key {
            "group" => {
                if v["id"].is_string() {
                    if let Some(g) = ctx.shared.groups.iter_mut().find(|g| g["id"] == self.id.as_str()) {
                        for k in ["name", "projectIds", "folder", "description", "instructions"] {
                            g[k] = v[k].clone();
                        }
                    }
                    self.group = v;
                }
            }
            "name" => {
                if let Some(n) = v.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    ctx.send("saved", Method::PATCH, self.path(), json!({ "name": n }));
                }
            }
            "description" | "instructions" => {
                if let Some(text) = v.as_str() {
                    ctx.send("saved", Method::PATCH, self.path(), json!({ key: text }));
                }
            }
            "folder" => {
                if let Some(f) = v.as_str() {
                    let folder = if f.trim().is_empty() { Value::Null } else { json!(f.trim()) };
                    ctx.send("saved", Method::PATCH, self.path(), json!({ "folder": folder }));
                }
            }
            "saved" => {
                ctx.info("saved");
                ctx.get("group", self.path());
            }
            "delete" if v == Value::Bool(true) => ctx.send("deleted", Method::DELETE, self.path(), Value::Null),
            "deleted" => {
                ctx.shared.groups.retain(|g| g["id"] != self.id.as_str());
                ctx.shared.sessions.retain(|s| s["groupId"] != self.id.as_str());
                ctx.info("group deleted");
                ctx.go(Route::Projects);
            }
            _ => {}
        }
        self.rebuild(ctx);
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        if self.form.rows.is_empty() {
            self.rebuild(ctx);
        }
        self.form.draw(f, area, "group settings — j/k move · Enter change / open");
    }
}
