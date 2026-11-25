//! A text input with a cursor: single line (prompts, filters) or multiline (the message box).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};

#[derive(Default, Clone)]
pub struct TextInput {
    text: Vec<char>,
    cursor: usize,
    pub multiline: bool,
    pub secret: bool,
    /// Previously submitted values (Up/Down on a single line or an empty box).
    history: Vec<String>,
    hist_pos: Option<usize>,
}

/// What a key did to the input.
#[derive(PartialEq, Eq, Debug)]
pub enum InputEvent {
    /// Enter (Alt+Enter / Shift+Enter add a newline in multiline mode).
    Submit,
    Cancel,
    Changed,
    /// Not an editing key.
    Ignored,
}

impl TextInput {
    pub fn new(value: &str) -> Self {
        let text: Vec<char> = value.chars().collect();
        Self { cursor: text.len(), text, ..Default::default() }
    }

    pub fn multiline() -> Self {
        Self { multiline: true, ..Default::default() }
    }

    pub fn value(&self) -> String {
        self.text.iter().collect()
    }

    pub fn set(&mut self, value: &str) {
        self.text = value.chars().collect();
        self.cursor = self.text.len();
    }

    pub fn clear(&mut self) {
        let v = self.value();
        if !v.trim().is_empty() && self.history.last() != Some(&v) {
            self.history.push(v);
        }
        self.hist_pos = None;
        self.text.clear();
        self.cursor = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.text.iter().all(|c| c.is_whitespace())
    }

    fn line_start(&self, at: usize) -> usize {
        self.text[..at].iter().rposition(|c| *c == '\n').map(|i| i + 1).unwrap_or(0)
    }

    fn line_end(&self, at: usize) -> usize {
        self.text[at..].iter().position(|c| *c == '\n').map(|i| at + i).unwrap_or(self.text.len())
    }

    fn word_left(&self) -> usize {
        let mut i = self.cursor;
        while i > 0 && self.text[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !self.text[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    fn recall(&mut self, older: bool) -> bool {
        if self.history.is_empty() || (self.multiline && !self.is_empty() && self.hist_pos.is_none()) {
            return false;
        }
        let n = self.history.len();
        let next = match (self.hist_pos, older) {
            (None, true) => Some(n - 1),
            (Some(0), true) => Some(0),
            (Some(i), true) => Some(i - 1),
            (Some(i), false) if i + 1 < n => Some(i + 1),
            _ => None,
        };
        self.hist_pos = next;
        let v = next.map(|i| self.history[i].clone()).unwrap_or_default();
        self.set(&v);
        true
    }

    pub fn on_key(&mut self, k: KeyEvent) -> InputEvent {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        match k.code {
            KeyCode::Enter if self.multiline && (alt || shift) => self.insert('\n'),
            KeyCode::Enter => return InputEvent::Submit,
            KeyCode::Esc => return InputEvent::Cancel,
            KeyCode::Char('u') if ctrl => {
                let s = self.line_start(self.cursor);
                self.text.drain(s..self.cursor);
                self.cursor = s;
            }
            KeyCode::Char('w') if ctrl => {
                let s = self.word_left();
                self.text.drain(s..self.cursor);
                self.cursor = s;
            }
            KeyCode::Char('a') if ctrl => self.cursor = self.line_start(self.cursor),
            KeyCode::Char('e') if ctrl => self.cursor = self.line_end(self.cursor),
            KeyCode::Char(c) if !ctrl => self.insert(c),
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.text.remove(self.cursor);
            }
            KeyCode::Delete if self.cursor < self.text.len() => {
                self.text.remove(self.cursor);
            }
            KeyCode::Left if ctrl || alt => self.cursor = self.word_left(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.text.len()),
            KeyCode::Home => self.cursor = self.line_start(self.cursor),
            KeyCode::End => self.cursor = self.line_end(self.cursor),
            KeyCode::Up => {
                let s = self.line_start(self.cursor);
                if s == 0 {
                    return if self.recall(true) { InputEvent::Changed } else { InputEvent::Ignored };
                }
                let col = self.cursor - s;
                let ps = self.line_start(s - 1);
                self.cursor = (ps + col).min(s - 1);
            }
            KeyCode::Down => {
                let e = self.line_end(self.cursor);
                if e == self.text.len() {
                    return if self.recall(false) { InputEvent::Changed } else { InputEvent::Ignored };
                }
                let col = self.cursor - self.line_start(self.cursor);
                let ne = self.line_end(e + 1);
                self.cursor = (e + 1 + col).min(ne);
            }
            _ => return InputEvent::Ignored,
        }
        InputEvent::Changed
    }

    fn insert(&mut self, c: char) {
        if c == '\n' && !self.multiline {
            return;
        }
        self.text.insert(self.cursor, c);
        self.cursor += 1;
    }

    /// Insert pasted text at the cursor (newlines kept only in multiline mode).
    pub fn paste(&mut self, s: &str) {
        for c in s.chars().filter(|c| *c != '\r') {
            self.insert(if c == '\n' && !self.multiline { ' ' } else { c });
        }
    }

    /// Lines needed to show the text at `width` (for sizing the box).
    pub fn height(&self, width: u16) -> u16 {
        let w = width.max(1) as usize;
        self.value().split('\n').map(|l| (l.chars().count() / w) as u16 + 1).sum::<u16>().max(1)
    }

    /// Draw inside `block`; the terminal cursor is placed when `focused`.
    pub fn draw(&self, f: &mut Frame, area: Rect, block: Block<'static>, focused: bool, placeholder: &str) {
        let inner = block.inner(area);
        let shown: String = if self.secret { "•".repeat(self.text.len()) } else { self.value() };
        let w = inner.width.max(1) as usize;
        // wrap by chars and find the cursor row/col
        let mut lines: Vec<String> = vec![];
        let (mut crow, mut ccol) = (0usize, 0usize);
        let mut idx = 0usize;
        for raw in shown.split('\n') {
            let chars: Vec<char> = raw.chars().collect();
            let chunks: Vec<String> =
                if chars.is_empty() { vec![String::new()] } else { chars.chunks(w).map(|c| c.iter().collect()).collect() };
            for (ci, chunk) in chunks.iter().enumerate() {
                let len = chunk.chars().count();
                if self.cursor >= idx && self.cursor <= idx + len && (self.cursor < idx + len || ci == chunks.len() - 1 || len < w) {
                    crow = lines.len();
                    ccol = self.cursor - idx;
                }
                lines.push(chunk.clone());
                idx += len;
            }
            idx += 1; // the newline
        }
        let height = inner.height as usize;
        let skip = crow.saturating_sub(height.saturating_sub(1));
        let visible: Vec<Line> = if self.text.is_empty() && !placeholder.is_empty() {
            vec![Line::styled(placeholder.to_string(), super::Theme::get().muted_style())]
        } else {
            lines.iter().skip(skip).take(height).map(|l| Line::raw(l.clone())).collect()
        };
        f.render_widget(Paragraph::new(visible).block(block), area);
        if focused {
            let x = inner.x + (ccol as u16).min(inner.width.saturating_sub(1));
            let y = inner.y + (crow - skip) as u16;
            f.set_cursor_position(Position::new(x, y.min(inner.y + inner.height.saturating_sub(1))));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn key(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: crossterm::event::KeyEventState::NONE }
    }

    #[test]
    fn edits_and_history() {
        let mut i = TextInput::multiline();
        for c in "hello world".chars() {
            i.on_key(key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        i.on_key(key(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(i.value(), "hello ");
        i.on_key(key(KeyCode::Enter, KeyModifiers::ALT));
        assert_eq!(i.value(), "hello \n");
        assert_eq!(i.on_key(key(KeyCode::Enter, KeyModifiers::NONE)), InputEvent::Submit);
        i.clear();
        assert!(i.is_empty());
        i.on_key(key(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(i.value(), "hello \n");
    }
}
