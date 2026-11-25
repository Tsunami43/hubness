//! A project (the web app's project workspace): Sessions, Files, Search, Git, Run, Shell,
//! Secrets and Settings tabs. Each tab is its own screen in this folder.

pub mod files;
pub mod git;
pub mod search;

use std::sync::{Arc, Mutex};

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use serde_json::Value;

pub mod form;
pub mod run;
pub mod secrets;
pub mod sessions;
pub mod settings;
pub mod shell;
pub mod termpane;

use super::tabs::{Tab, Tabs};
use crate::core::{Action, Ctx, Handled, Screen};
use crate::ui::Theme;

pub struct ProjectScreen {
    id: String,
    project: Value,
    tabs: Tabs,
    opener: Opener,
}

/// A request from one tab (Search, Git) to show a file in the Files tab, at a line.
#[derive(Clone, Default)]
pub struct Opener(Arc<Mutex<Option<OpenRequest>>>);

/// A file path and an optional 1-based line.
pub type OpenRequest = (String, Option<usize>);

impl Opener {
    pub fn request(&self, path: &str, line: Option<usize>) {
        *self.0.lock().expect("opener lock") = Some((path.to_string(), line));
    }
    pub fn take(&self) -> Option<OpenRequest> {
        self.0.lock().expect("opener lock").take()
    }
    fn pending(&self) -> bool {
        self.0.lock().expect("opener lock").is_some()
    }
}

/// Percent-encode a query value or path segment.
pub fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl ProjectScreen {
    pub fn new(id: &str) -> Self {
        let opener = Opener::default();
        let tabs = vec![
            Tab::new("sessions", "Sessions", Box::new(sessions::SessionsTab::new(sessions::Scope::Project(id.to_string())))),
            Tab::new("files", "Files", Box::new(files::FilesTab::new(id, opener.clone()))),
            Tab::new("search", "Search", Box::new(search::SearchTab::new(id, opener.clone()))),
            Tab::new("git", "Git", Box::new(git::GitTab::new(id, opener.clone()))),
            Tab::new("run", "Run", Box::new(run::RunTab::new(id))),
            Tab::new("shell", "Shell", Box::new(shell::ShellTab::project(id))),
            Tab::new("secrets", "Secrets", Box::new(secrets::SecretsTab::new(id))),
            Tab::new("settings", "Settings", Box::new(settings::SettingsTab::new(id))),
        ];
        Self { id: id.to_string(), project: Value::Null, tabs: Tabs::new(tabs), opener }
    }

    /// A tab asked to open a file: switch to the Files tab (it takes the request on show).
    fn follow_opener(&mut self, ctx: &mut Ctx) {
        if self.opener.pending() {
            self.tabs.select_id("files", ctx);
        }
    }
}

impl Screen for ProjectScreen {
    fn title(&self) -> String {
        self.project["name"].as_str().unwrap_or("Project").to_string()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        ctx.get("project", format!("/projects/{}", self.id));
        self.tabs.on_show(ctx);
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = Theme::get();
        let [head, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
        let p = &self.project;
        // the shared list is kept fresh by the Settings tab (rename)
        let name = ctx
            .shared
            .projects
            .iter()
            .find(|x| x["id"] == self.id.as_str())
            .and_then(|x| x["name"].as_str())
            .or(p["name"].as_str())
            .unwrap_or("…");
        let repo = &p["repo"];
        let branch = repo["branch"].as_str().map(|b| format!("  {b}")).unwrap_or_default();
        let provider = repo["providerLabel"].as_str().map(|l| format!(" · {l}")).unwrap_or_default();
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {}", name), t.bold()),
                Span::styled(format!("{branch}{provider}   {}", p["path"].as_str().unwrap_or_default()), t.muted_style()),
            ])),
            head,
        );
        self.tabs.draw(f, body, ctx);
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        let r = self.tabs.on_key(k, ctx);
        self.follow_opener(ctx);
        r
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        if let Some(result) = self.tabs.on_task(key, result, ctx)
            && key == "project"
            && let Some(v) = ctx.ok(result)
        {
            self.project = v;
        }
        self.follow_opener(ctx);
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

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        self.tabs.on_action(key, ctx);
        self.follow_opener(ctx);
    }
}
