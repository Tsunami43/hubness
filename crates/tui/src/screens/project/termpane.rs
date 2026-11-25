//! A job terminal inside a tab (Run output, shells): attaches a `JobTerm` to its stream and
//! sends keys to the job in order — one input request in flight, the rest queued.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::Rect;
use serde_json::{Value, json};

use crate::core::Ctx;
use crate::ui::block;
use crate::ui::term::JobTerm;

/// The key that leaves an interactive terminal (everything else goes to the job).
pub const LEAVE_HINT: &str = "Ctrl+]";

/// Ctrl+] (terminals report it as Ctrl+5 without the kitty keyboard protocol).
pub fn is_leave(k: &KeyEvent) -> bool {
    k.modifiers.contains(KeyModifiers::CONTROL) && matches!(k.code, KeyCode::Char(']') | KeyCode::Char('5'))
}

/// Until the pane is drawn: the terminal minus the sidebar, header, tab bar and footers.
fn estimated_area() -> Rect {
    let (w, h) = crossterm::terminal::size().unwrap_or((130, 40));
    let w = if w > 70 { w - 30 } else { w };
    Rect::new(0, 0, w, h.saturating_sub(5))
}

pub struct TermPane {
    /// Task / stream key prefix (several panes can live in one screen).
    key: String,
    pub term: Option<JobTerm>,
    pending: String,
    in_flight: bool,
    /// Where the pane was drawn last (the pty size for new jobs).
    area: Rect,
}

impl TermPane {
    pub fn new(key: &str) -> Self {
        Self { key: key.to_string(), term: None, pending: String::new(), in_flight: false, area: estimated_area() }
    }

    pub fn job_id(&self) -> Option<&str> {
        self.term.as_ref().map(|t| t.id.as_str())
    }

    pub fn running(&self) -> bool {
        self.term.as_ref().is_some_and(|t| t.running)
    }

    /// `{cols, rows}` for starting a job that will be shown in this pane.
    pub fn size_body(&self) -> Value {
        let (cols, rows) = JobTerm::size_for(self.area);
        json!({ "cols": cols, "rows": rows })
    }

    /// Show `job` (a JobInfo) and follow its output; the same job is not re-attached.
    pub fn attach(&mut self, job: &Value, ctx: &mut Ctx) {
        let id = job["id"].as_str().unwrap_or_default();
        if id.is_empty() || self.job_id() == Some(id) {
            return;
        }
        let term = JobTerm::new(job);
        ctx.stream(self.stream_key(), term.stream_path());
        self.term = Some(term);
        self.pending.clear();
        self.in_flight = false;
    }

    pub fn detach(&mut self, ctx: &mut Ctx) {
        if self.term.take().is_some() {
            ctx.close_stream(&self.stream_key());
        }
    }

    fn stream_key(&self) -> String {
        format!("{}:stream", self.key)
    }

    fn input_key(&self) -> String {
        format!("{}:input", self.key)
    }

    /// A stream message for this pane: `Some(exited)` when it was ours.
    pub fn on_stream(&mut self, key: &str, msg: &Value) -> Option<bool> {
        if key != self.stream_key() {
            return None;
        }
        let term = self.term.as_mut()?;
        if msg["type"] == "__ended" {
            return Some(false);
        }
        term.on_stream(msg);
        Some(msg["type"] == "exit")
    }

    /// A task result for this pane (the input queue); true when it was ours.
    pub fn on_task(&mut self, key: &str, result: &anyhow::Result<Value>, ctx: &mut Ctx) -> bool {
        if key != self.input_key() {
            return false;
        }
        self.in_flight = false;
        if let Err(e) = result {
            ctx.error(e);
            self.pending.clear();
        }
        self.flush(ctx);
        true
    }

    /// Send a key to the job.
    pub fn key(&mut self, k: KeyEvent, ctx: &mut Ctx) {
        if let Some(bytes) = JobTerm::key_bytes(k) {
            self.input(&bytes, ctx);
        }
    }

    /// Send text (keys, a paste) to the job.
    pub fn input(&mut self, data: &str, ctx: &mut Ctx) {
        if !self.running() {
            return;
        }
        self.pending.push_str(data);
        self.flush(ctx);
    }

    fn flush(&mut self, ctx: &mut Ctx) {
        if self.in_flight || self.pending.is_empty() {
            return;
        }
        let Some(id) = self.job_id().map(str::to_string) else { return };
        let data = std::mem::take(&mut self.pending);
        self.in_flight = true;
        ctx.send(self.input_key(), Method::POST, format!("/jobs/{id}/input"), json!({ "data": data }));
    }

    /// Draw the terminal (or `empty` when no job is attached).
    pub fn draw(&mut self, f: &mut Frame, area: Rect, title: &str, focused: bool, empty: &str) {
        self.area = area;
        match &mut self.term {
            Some(term) => {
                let state = match (term.running, term.exit_code) {
                    (true, _) => "running".to_string(),
                    (false, Some(0)) => "exited".to_string(),
                    (false, Some(c)) => format!("failed, exit code {c}"),
                    (false, None) => "exited".to_string(),
                };
                let hint = if focused { format!(" — typing into the terminal, {LEAVE_HINT} to leave") } else { String::new() };
                term.draw(f, area, block(format!("{title} · {state}{hint}"), focused));
            }
            None => f.render_widget(ratatui::widgets::Paragraph::new(empty.to_string()).block(block(title, false)), area),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leave_key() {
        assert!(is_leave(&KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL)));
        assert!(is_leave(&KeyEvent::new(KeyCode::Char('5'), KeyModifiers::CONTROL)));
        assert!(!is_leave(&KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE)));
    }
}
