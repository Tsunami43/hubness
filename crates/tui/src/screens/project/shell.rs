//! The Shell tab of a project or a group (the web app's ShellPanel): several shells as
//! tabs, each a full interactive terminal; reconnects to the open ones, opens another one,
//! closes (kills) one.

use crossterm::event::{KeyCode, KeyEvent};
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use serde_json::{Value, json};

use super::termpane::{LEAVE_HINT, TermPane, is_leave};
use crate::core::{Action, Ctx, Handled, Screen};
use crate::ui::Theme;

pub struct ShellTab {
    /// `/projects/:id` or `/groups/:id`.
    base: String,
    /// Open shells (JobInfo) in tab order.
    jobs: Vec<Value>,
    active: usize,
    term: TermPane,
    typing: bool,
    loaded: bool,
}

impl ShellTab {
    pub fn project(id: &str) -> Self {
        Self::new(format!("/projects/{id}"))
    }

    pub fn group(id: &str) -> Self {
        Self::new(format!("/groups/{id}"))
    }

    fn new(base: String) -> Self {
        Self { base, jobs: vec![], active: 0, term: TermPane::new("sh"), typing: false, loaded: false }
    }

    /// Open a shell; `fresh` — a new one (asked for: typing goes to it), otherwise reconnect to
    /// the existing one when the tab is shown (stays in NORMAL so tab motions keep working).
    fn open(&self, fresh: bool, ctx: &mut Ctx) {
        let mut body = self.term.size_body();
        body["fresh"] = json!(fresh);
        ctx.send(if fresh { "opened" } else { "opened-auto" }, Method::POST, format!("{}/shell", self.base), body);
    }

    fn show_active(&mut self, ctx: &mut Ctx) {
        match self.jobs.get(self.active).cloned() {
            Some(job) => {
                if self.term.job_id() != job["id"].as_str() {
                    self.term.detach(ctx);
                    self.term.attach(&job, ctx);
                }
            }
            None => {
                self.term.detach(ctx);
                self.typing = false;
            }
        }
    }

    fn switch(&mut self, delta: isize, ctx: &mut Ctx) {
        let n = self.jobs.len() as isize;
        if n == 0 {
            return;
        }
        self.active = ((self.active as isize + delta).rem_euclid(n)) as usize;
        self.show_active(ctx);
    }

    fn close(&mut self, ctx: &mut Ctx) {
        let Some(job) = self.jobs.get(self.active).cloned() else { return };
        let id = job["id"].as_str().unwrap_or_default().to_string();
        ctx.send("killed", Method::POST, format!("/jobs/{id}/kill"), Value::Null);
        self.jobs.remove(self.active);
        self.active = self.active.min(self.jobs.len().saturating_sub(1));
        self.show_active(ctx);
    }
}

impl Screen for ShellTab {
    fn title(&self) -> String {
        "Shell".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        ctx.get("shells", format!("{}/shells", self.base));
    }

    fn captures_text(&self) -> bool {
        self.typing
    }

    fn actions(&self) -> Vec<Action> {
        vec![
            Action::new("i", format!("Type into the shell ({LEAVE_HINT} leaves)")),
            Action::new("n", "New shell (the open ones stay)"),
            Action::new("x", "Close (kill) this shell"),
            Action::new("]", "Next shell"),
            Action::new("[", "Previous shell"),
            Action::new("R", "Reconnect / reload"),
        ]
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        match key {
            "i" => {
                if self.term.running() {
                    self.typing = true;
                } else {
                    ctx.info("no running shell — n opens one");
                }
            }
            "n" => self.open(true, ctx),
            "x" => self.close(ctx),
            "]" => self.switch(1, ctx),
            "[" => self.switch(-1, ctx),
            "R" => self.on_show(ctx),
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        if self.typing {
            if is_leave(&k) || !self.term.running() {
                self.typing = false;
            } else {
                self.term.key(k, ctx);
            }
            return Handled::Yes;
        }
        match k.code {
            KeyCode::Enter => {
                if self.term.running() {
                    self.typing = true;
                } else {
                    self.open(true, ctx);
                }
            }
            KeyCode::Right | KeyCode::Char('l') => self.switch(1, ctx),
            KeyCode::Left | KeyCode::Char('h') => self.switch(-1, ctx),
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn on_paste(&mut self, text: &str, ctx: &mut Ctx) {
        if self.typing {
            self.term.input(text, ctx);
        }
    }

    fn on_stream(&mut self, key: &str, msg: Value, _ctx: &mut Ctx) {
        if self.term.on_stream(key, &msg) == Some(true) {
            self.typing = false;
            if let Some(j) = self.jobs.get_mut(self.active) {
                j["status"] = json!("exited");
            }
        }
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        if self.term.on_task(key, &result, ctx) {
            return;
        }
        let Some(v) = ctx.ok(result) else { return };
        match key {
            "shells" => {
                let list = v.as_array().cloned().unwrap_or_default();
                if list.is_empty() && !self.loaded {
                    // the first visit opens a shell (as the web app does)
                    self.open(false, ctx);
                }
                self.loaded = true;
                // keep the current one active if it is still open
                let current = self.jobs.get(self.active).map(|j| j["id"].clone());
                self.jobs = list;
                self.active = current.and_then(|id| self.jobs.iter().position(|j| j["id"] == id)).unwrap_or(0);
                self.show_active(ctx);
            }
            "opened" | "opened-auto" => {
                match self.jobs.iter().position(|j| j["id"] == v["id"]) {
                    Some(i) => self.active = i,
                    None => {
                        self.jobs.push(v);
                        self.active = self.jobs.len() - 1;
                    }
                }
                self.show_active(ctx);
                self.typing = key == "opened";
            }
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        let t = Theme::get();
        let [bar, body, foot] = Layout::vertical([Constraint::Length(1), Constraint::Min(3), Constraint::Length(1)]).areas(area);
        let mut spans = vec![];
        for (i, j) in self.jobs.iter().enumerate() {
            let running = j["status"] != "exited";
            spans.push(Span::styled(if running { " ●" } else { " ○" }, Style::default().fg(if running { t.ok } else { t.muted })));
            let label = format!(" {} ", j["title"].as_str().unwrap_or("Shell"));
            spans.push(Span::styled(label, if i == self.active { t.key_style() } else { t.muted_style() }));
        }
        spans.push(Span::styled("   n new · x close · [ ] switch", t.muted_style()));
        f.render_widget(Paragraph::new(Line::from(spans)), bar);
        let title = self.jobs.get(self.active).and_then(|j| j["title"].as_str()).unwrap_or("Shell").to_string();
        self.term.draw(f, body, &title, self.typing, "No shell open — n (or Enter) opens one.");
        let hint = if self.typing {
            format!("{LEAVE_HINT} leaves the terminal · keys go to the shell")
        } else {
            "Enter / i types into the shell · a shell nobody watches closes after 30 minutes".to_string()
        };
        f.render_widget(Paragraph::new(Span::styled(hint, t.muted_style())), foot);
    }
}
