//! A group of projects (the web app's group page): Sessions, Repositories, Shell and Settings

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use serde_json::Value;

pub mod repos;
pub mod sessions;
pub mod settings;
pub mod shell;

use super::tabs::{Tab, Tabs};
use crate::core::{Action, Ctx, Handled, Screen};
use crate::ui::Theme;

pub struct GroupScreen {
    id: String,
    group: Value,
    tabs: Tabs,
}

impl GroupScreen {
    pub fn new(id: &str) -> Self {
        let tabs = vec![
            Tab::new("sessions", "Sessions", Box::new(sessions::tab(id))),
            Tab::new("repos", "Repositories", Box::new(repos::ReposTab::new(id))),
            Tab::new("shell", "Shell", Box::new(shell::tab(id))),
            Tab::new("settings", "Settings", Box::new(settings::SettingsTab::new(id))),
        ];
        Self { id: id.to_string(), group: Value::Null, tabs: Tabs::new(tabs) }
    }
}

impl Screen for GroupScreen {
    fn title(&self) -> String {
        self.group["name"].as_str().unwrap_or("Group").to_string()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        ctx.get("group", format!("/groups/{}", self.id));
        self.tabs.on_show(ctx);
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = Theme::get();
        let [head, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
        // the shared list is kept fresh by the tabs (rename, members)
        let p = ctx.shared.groups.iter().find(|g| g["id"] == self.id.as_str()).unwrap_or(&self.group);
        let repos = format!("  {} repositories", p["projectIds"].as_array().map(|a| a.len()).unwrap_or(0));
        let path = self.group["workspace"]["path"].as_str().or(p["folder"].as_str()).unwrap_or_default();
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {}", p["name"].as_str().unwrap_or("…")), t.bold()),
                Span::styled(format!("{repos}   {path}"), t.muted_style()),
            ])),
            head,
        );
        self.tabs.draw(f, body, ctx);
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        self.tabs.on_key(k, ctx)
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        if let Some(result) = self.tabs.on_task(key, result, ctx)
            && key == "group"
            && let Some(v) = ctx.ok(result)
        {
            self.group = v;
        }
    }

    fn on_stream(&mut self, key: &str, msg: Value, ctx: &mut Ctx) {
        self.tabs.on_stream(key, msg, ctx);
    }

    fn on_paste(&mut self, text: &str, ctx: &mut Ctx) {
        self.tabs.on_paste(text, ctx);
    }

    fn captures_text(&self) -> bool {
        self.tabs.captures_text()
    }

    fn actions(&self) -> Vec<Action> {
        self.tabs.actions()
    }

    fn tab_names(&self) -> Vec<String> {
        self.tabs.names()
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        self.tabs.on_action(key, ctx);
    }
}
