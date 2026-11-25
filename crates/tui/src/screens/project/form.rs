//! A settings page as a list of rows: section headings, read-only text and selectable
//! fields that carry an action (Enter runs it). Used by the project and group Settings tabs.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::ui::{Theme, block};

pub enum Row<A> {
    Head(String),
    Text(String),
    Field { label: String, value: Line<'static>, act: Option<A> },
}

pub struct Form<A> {
    pub rows: Vec<Row<A>>,
    /// The selected row (always a field with an action when there is one).
    sel: usize,
    scroll: usize,
}

impl<A: Clone> Default for Form<A> {
    fn default() -> Self {
        Self { rows: vec![], sel: 0, scroll: 0 }
    }
}

const LABEL: usize = 20;

impl<A: Clone + PartialEq> Form<A> {
    /// Replace the rows, keeping the selection on the same action when it is still there.
    pub fn set(&mut self, rows: Vec<Row<A>>) {
        let keep = self.current();
        self.rows = rows;
        let found = keep.and_then(|a| self.rows.iter().position(|r| matches!(r, Row::Field { act: Some(x), .. } if *x == a)));
        self.sel = found.unwrap_or(self.sel);
        if !self.selectable(self.sel) {
            self.step(1);
        }
    }

    fn selectable(&self, i: usize) -> bool {
        matches!(self.rows.get(i), Some(Row::Field { act: Some(_), .. }))
    }

    /// Move the selection by `delta` selectable rows.
    pub fn step(&mut self, delta: isize) {
        let n = self.rows.len() as isize;
        if n == 0 {
            return;
        }
        let dir = if delta < 0 { -1 } else { 1 };
        let mut i = self.sel as isize;
        let mut best = None;
        let mut left = delta.abs().max(1);
        loop {
            i += dir;
            if i < 0 || i >= n {
                break;
            }
            if self.selectable(i as usize) {
                best = Some(i as usize);
                left -= 1;
                if left == 0 {
                    break;
                }
            }
        }
        match best {
            Some(b) => self.sel = b,
            None if !self.selectable(self.sel) => {
                // nothing selectable in that direction: take the first selectable at all
                if let Some(first) = (0..self.rows.len()).find(|&j| self.selectable(j)) {
                    self.sel = first;
                }
            }
            None => {}
        }
    }

    pub fn current(&self) -> Option<A> {
        match self.rows.get(self.sel) {
            Some(Row::Field { act: Some(a), .. }) => Some(a.clone()),
            _ => None,
        }
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, title: &str) {
        let t = Theme::get();
        let b = block(title, true);
        let inner = b.inner(area);
        f.render_widget(b, area);
        let width = inner.width as usize;
        let mut lines: Vec<Line<'static>> = vec![];
        let mut sel_line = 0;
        for (i, row) in self.rows.iter().enumerate() {
            match row {
                Row::Head(h) => {
                    if !lines.is_empty() {
                        lines.push(Line::raw(""));
                    }
                    lines.push(Line::styled(format!(" {}", h.to_uppercase()), t.accent_style()));
                }
                Row::Text(text) => {
                    for l in text.lines() {
                        for w in wrap_words(l, width.saturating_sub(3).max(10)) {
                            lines.push(Line::styled(format!("   {w}"), t.muted_style()));
                        }
                    }
                }
                Row::Field { label, value, act } => {
                    let selected = i == self.sel && act.is_some();
                    if selected {
                        sel_line = lines.len();
                    }
                    let marker = if selected { " › " } else { "   " };
                    let mut spans = vec![
                        Span::styled(marker, t.accent_style()),
                        Span::styled(format!("{label:<LABEL$} "), if act.is_some() { Style::default() } else { t.muted_style() }),
                    ];
                    spans.extend(value.spans.iter().cloned());
                    let mut line = Line::from(spans);
                    if selected {
                        line = line.style(t.selected());
                    }
                    lines.push(line);
                }
            }
        }
        let h = inner.height as usize;
        if sel_line < self.scroll {
            self.scroll = sel_line.saturating_sub(1);
        } else if sel_line >= self.scroll + h {
            self.scroll = sel_line + 1 - h;
        }
        self.scroll = self.scroll.min(lines.len().saturating_sub(h));
        let visible: Vec<Line> = lines.into_iter().skip(self.scroll).take(h).collect();
        f.render_widget(Paragraph::new(visible), inner);
    }
}

/// A field value: plain text, or muted when empty.
pub fn value(s: &str, empty: &str) -> Line<'static> {
    let t = Theme::get();
    let first = s.lines().next().unwrap_or_default();
    if first.trim().is_empty() {
        return Line::styled(empty.to_string(), t.muted_style());
    }
    let more = s.lines().count();
    if more > 1 {
        Line::from(vec![Span::raw(first.to_string()), Span::styled(format!("  … ({more} lines)"), t.muted_style())])
    } else {
        Line::raw(first.to_string())
    }
}

/// Word-wrap a line to `width` columns (words longer than a line, like paths, are split).
pub fn wrap_words(s: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = vec![];
    let mut cur = String::new();
    for word in s.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        if !cur.is_empty() && cur.chars().count() + 1 + word.len() > width {
            out.push(std::mem::take(&mut cur));
        }
        // a word longer than a line never shares one (cur is empty here)
        while word.len() > width {
            let rest = word.split_off(width);
            out.push(word.into_iter().collect());
            word = rest;
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.extend(word);
    }
    if !cur.is_empty() || out.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(a: u8) -> Row<u8> {
        Row::Field { label: "x".into(), value: Line::raw(""), act: Some(a) }
    }

    #[test]
    fn selection_skips_headings_and_text() {
        let mut f: Form<u8> = Form::default();
        f.set(vec![Row::Head("a".into()), field(1), Row::Text("t".into()), field(2), Row::Head("b".into()), field(3)]);
        assert_eq!(f.current(), Some(1));
        f.step(1);
        assert_eq!(f.current(), Some(2));
        f.step(5);
        assert_eq!(f.current(), Some(3));
        f.step(-1);
        assert_eq!(f.current(), Some(2));
        // keeps the selected action across a rebuild
        f.set(vec![Row::Head("a".into()), field(2), field(3)]);
        assert_eq!(f.current(), Some(2));
    }

    #[test]
    fn wraps_words() {
        assert_eq!(wrap_words("one two three", 7), vec!["one two", "three"]);
        assert_eq!(wrap_words("", 7), vec![""]);
        assert_eq!(wrap_words("at /a/very/long/path end", 6), vec!["at", "/a/ver", "y/long", "/path", "end"]);
    }
}
