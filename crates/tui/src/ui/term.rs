//! A terminal job (shell, Run command, install/login, clone) shown inside the TUI: the
//! job's pty output goes through a vt100 parser and is drawn with tui-term; keys are sent to
//! the job as terminal input (`POST /api/jobs/:id/input`).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::Block;
use serde_json::Value;
use tui_term::widget::PseudoTerminal;

pub struct JobTerm {
    pub id: String,
    pub title: String,
    parser: vt100::Parser,
    pub running: bool,
    pub exit_code: Option<i64>,
    size: (u16, u16),
}

impl JobTerm {
    pub fn new(job: &Value) -> Self {
        Self {
            id: job["id"].as_str().unwrap_or_default().to_string(),
            title: job["title"].as_str().unwrap_or("job").to_string(),
            parser: vt100::Parser::new(30, 100, 5000),
            running: job["status"] != "exited",
            exit_code: job["exitCode"].as_i64(),
            size: (30, 100),
        }
    }

    /// The SSE path of the job's output.
    pub fn stream_path(&self) -> String {
        format!("/jobs/{}/stream", self.id)
    }

    /// A message of `/jobs/:id/stream` (`snapshot`, `output`, `exit`).
    pub fn on_stream(&mut self, msg: &Value) {
        match msg["type"].as_str() {
            Some("snapshot") => {
                self.parser = vt100::Parser::new(self.size.0, self.size.1, 5000);
                self.parser.process(msg["output"].as_str().unwrap_or_default().as_bytes());
                self.running = msg["job"]["status"] != "exited";
                self.exit_code = msg["job"]["exitCode"].as_i64();
            }
            Some("output") => self.parser.process(msg["data"].as_str().unwrap_or_default().as_bytes()),
            Some("exit") => {
                self.running = false;
                self.exit_code = msg["code"].as_i64();
            }
            _ => {}
        }
    }

    /// Draw the screen; the parser follows the area's size.
    pub fn draw(&mut self, f: &mut Frame, area: Rect, block: Block<'static>) {
        let inner = block.inner(area);
        let size = (inner.height.max(1), inner.width.max(1));
        if size != self.size {
            self.size = size;
            self.parser.screen_mut().set_size(size.0, size.1);
        }
        f.render_widget(PseudoTerminal::new(self.parser.screen()).block(block), area);
    }

    /// The pty size to request when starting a job in this area.
    pub fn size_for(area: Rect) -> (u16, u16) {
        (area.width.saturating_sub(2).max(20), area.height.saturating_sub(2).max(5))
    }

    /// Bytes a key sends to a terminal (`None` for keys the TUI keeps).
    pub fn key_bytes(k: KeyEvent) -> Option<String> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let s = match k.code {
            KeyCode::Char(c) if ctrl => {
                let b = (c.to_ascii_lowercase() as u8).wrapping_sub(b'a').wrapping_add(1);
                if (1..=26).contains(&b) { (b as char).to_string() } else { return None }
            }
            KeyCode::Char(c) if alt => format!("\x1b{c}"),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Enter => "\r".into(),
            KeyCode::Backspace => "\x7f".into(),
            KeyCode::Tab => "\t".into(),
            KeyCode::BackTab => "\x1b[Z".into(),
            KeyCode::Esc => "\x1b".into(),
            KeyCode::Up => "\x1b[A".into(),
            KeyCode::Down => "\x1b[B".into(),
            KeyCode::Right => "\x1b[C".into(),
            KeyCode::Left => "\x1b[D".into(),
            KeyCode::Home => "\x1b[H".into(),
            KeyCode::End => "\x1b[F".into(),
            KeyCode::Delete => "\x1b[3~".into(),
            KeyCode::PageUp => "\x1b[5~".into(),
            KeyCode::PageDown => "\x1b[6~".into(),
            _ => return None,
        };
        Some(s)
    }
}
