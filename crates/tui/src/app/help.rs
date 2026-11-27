//! The `?` help: every key that works on the current screen — its actions, the motions,
//! panes and tabs, global keys and the leader menu — in one scrollable, filterable panel.
//! The rows come from the screen's `actions()` and the tables in `keymap`, so the help shows
//! exactly the keys that are handled.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::core::Action;
use crate::keymap::{self, Group};
use crate::screens::kit;
use crate::ui::{Theme, block, centered};

/// A row of the help.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Header(String),
    /// Keys, what they do, and whether `Enter` runs it (a screen action).
    Item {
        keys: String,
        label: String,
        action: bool,
    },
}

/// The help's rows: the screen's actions first, then the app-wide keys.
pub fn rows(screen: &str, actions: &[Action], leader: &[(String, String)]) -> Vec<Row> {
    let mut out = vec![];
    let item = |keys: String, label: String, action: bool| Row::Item { keys, label, action };
    if !actions.is_empty() {
        out.push(Row::Header(format!("{screen} — actions")));
        out.extend(actions.iter().map(|a| item(a.key.to_string(), a.label.clone(), true)));
    }
    for g in [Group::Motion, Group::Panes, Group::Global] {
        out.push(Row::Header(keymap::group_title(g).into()));
        out.extend(keymap::rows(g).into_iter().map(|(k, l)| item(k, l, false)));
    }
    if !leader.is_empty() {
        out.push(Row::Header("Leader: Space, then".into()));
        out.extend(leader.iter().map(|(k, l)| item(k.clone(), l.clone(), false)));
    }
    out
}

/// Rows matching the filter: items whose keys or label match, under their headers.
pub fn visible(rows: &[Row], filter: &str) -> Vec<Row> {
    if filter.trim().is_empty() {
        return rows.to_vec();
    }
    let mut out = vec![];
    let mut header: Option<&Row> = None;
    for r in rows {
        match r {
            Row::Header(_) => header = Some(r),
            Row::Item { keys, label, .. } => {
                if kit::matches(&format!("{keys} {label}"), filter) {
                    if let Some(h) = header.take() {
                        out.push(h.clone());
                    }
                    out.push(r.clone());
                }
            }
        }
    }
    out
}

/// What a key did to the help.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Stay,
    Close,
    /// Close and run this screen action.
    Run(String),
}

#[derive(Default)]
pub struct Help {
    pub rows: Vec<Row>,
    pub filter: String,
    /// Typing into the filter (after `/`).
    pub typing: bool,
    /// The selected item among the visible items (headers are skipped).
    pub selected: usize,
    /// The first visible row (kept so the selection stays in view).
    offset: usize,
    /// Rows that fit, from the last draw (for half-page moves).
    page: usize,
    /// `g` was pressed (for `gg`).
    g: bool,
}

impl Help {
    pub fn new(rows: Vec<Row>) -> Self {
        Self { rows, page: 10, ..Default::default() }
    }

    fn items(&self) -> Vec<Row> {
        visible(&self.rows, &self.filter).into_iter().filter(|r| matches!(r, Row::Item { .. })).collect()
    }

    fn step(&mut self, by: isize) {
        let n = self.items().len();
        if n == 0 {
            self.selected = 0;
            return;
        }
        self.selected = (self.selected as isize + by).clamp(0, n as isize - 1) as usize;
    }

    /// Pasted text: into the filter while it is typed, nowhere otherwise.
    pub fn on_paste(&mut self, text: &str) {
        if self.typing {
            self.filter.push_str(text);
            self.selected = 0;
        }
    }

    pub fn on_key(&mut self, k: KeyEvent) -> Outcome {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let half = (self.page / 2).max(1) as isize;
        if self.typing {
            match k.code {
                KeyCode::Esc => {
                    self.typing = false;
                    self.filter.clear();
                }
                KeyCode::Enter => self.typing = false,
                KeyCode::Backspace => {
                    if self.filter.pop().is_none() {
                        self.typing = false;
                    }
                }
                KeyCode::Down => self.step(1),
                KeyCode::Up => self.step(-1),
                KeyCode::Char('n') if ctrl => self.step(1),
                KeyCode::Char('p') if ctrl => self.step(-1),
                KeyCode::Char('u') if ctrl => self.filter.clear(),
                KeyCode::Char(c) if !ctrl => {
                    self.filter.push(c);
                    self.selected = 0;
                }
                _ => {}
            }
            self.step(0);
            return Outcome::Stay;
        }
        let g = std::mem::take(&mut self.g);
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => return Outcome::Close,
            KeyCode::Char('c') if ctrl => return Outcome::Close,
            KeyCode::Down | KeyCode::Char('j') => self.step(1),
            KeyCode::Up | KeyCode::Char('k') => self.step(-1),
            KeyCode::Char('n') if ctrl => self.step(1),
            KeyCode::Char('p') if ctrl => self.step(-1),
            KeyCode::Char('d') if ctrl => self.step(half),
            KeyCode::Char('u') if ctrl => self.step(-half),
            KeyCode::PageDown => self.step(self.page as isize),
            KeyCode::PageUp => self.step(-(self.page as isize)),
            KeyCode::Home => self.selected = 0,
            KeyCode::Char('g') if g => self.selected = 0,
            KeyCode::Char('g') => self.g = true,
            KeyCode::End | KeyCode::Char('G') => self.step(isize::MAX / 2),
            KeyCode::Char('/') => self.typing = true,
            KeyCode::Enter => {
                if let Some(Row::Item { keys, action: true, .. }) = self.items().get(self.selected) {
                    return Outcome::Run(keys.clone());
                }
            }
            // a screen action's key runs it right from the help
            KeyCode::Char(c) if !ctrl => {
                let key = c.to_string();
                if self.rows.iter().any(|r| matches!(r, Row::Item { keys, action: true, .. } if *keys == key)) {
                    return Outcome::Run(key);
                }
            }
            _ => {}
        }
        Outcome::Stay
    }

    /// Scroll one notch of the mouse wheel.
    pub fn wheel(&mut self, up: bool) {
        self.step(if up { -3 } else { 3 });
    }

    /// Draw over `area`; returns where the panel is.
    pub fn draw(&mut self, f: &mut Frame, area: Rect, title: &str, about: Option<&str>) -> Rect {
        let t = Theme::get();
        let w = area.width.saturating_sub(4).min(88);
        let h = area.height.saturating_sub(2).max(6);
        let r = centered(area, w, h);
        f.render_widget(Clear, r);
        let hint = if self.typing {
            " Enter keep · Esc clear · ↑↓ move "
        } else {
            " j/k move · / filter · Enter run an action · Esc close "
        };
        let b = block(format!("{}  Keys · {title}", crate::ui::theme::icon("\u{f030c}", "?")), true)
            .title_bottom(Line::styled(hint, t.muted_style()).right_aligned())
            .style(Style::default().bg(t.bg_dark));
        let inner = b.inner(r);
        f.render_widget(b, r);
        let about_h = u16::from(about.is_some());
        let [about_area, filter_area, list] =
            Layout::vertical([Constraint::Length(about_h), Constraint::Length(1), Constraint::Min(1)]).areas(inner);
        if let Some(a) = about {
            f.render_widget(Paragraph::new(Line::styled(format!(" {a}"), t.dim_style())), about_area);
        }
        let filter_line = if self.typing || !self.filter.is_empty() {
            Line::from(vec![Span::styled(" / ", t.accent_style()), Span::raw(self.filter.clone())])
        } else {
            Line::styled(" / to filter", t.muted_style())
        };
        f.render_widget(Paragraph::new(filter_line), filter_area);
        if self.typing {
            let x = filter_area.x + 3 + self.filter.chars().count() as u16;
            f.set_cursor_position((x.min(filter_area.right().saturating_sub(1)), filter_area.y));
        }
        let rows = visible(&self.rows, &self.filter);
        let key_w = rows
            .iter()
            .filter_map(|r| match r {
                Row::Item { keys, .. } => Some(keys.chars().count()),
                _ => None,
            })
            .max()
            .unwrap_or(4)
            .clamp(4, 20);
        // the row index of the selected item
        let mut sel_row = 0;
        let mut n = 0;
        for (i, r) in rows.iter().enumerate() {
            if matches!(r, Row::Item { .. }) {
                if n == self.selected {
                    sel_row = i;
                }
                n += 1;
            }
        }
        let height = list.height as usize;
        self.page = height.max(2);
        if sel_row < self.offset {
            // show the header above the first item of a group too
            self.offset = sel_row.saturating_sub(1);
        } else if sel_row >= self.offset + height {
            self.offset = sel_row + 1 - height;
        }
        self.offset = self.offset.min(rows.len().saturating_sub(height));
        let lines: Vec<Line> = rows
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(height)
            .map(|(i, r)| match r {
                Row::Header(h) => Line::styled(format!(" {h}"), t.accent_style().add_modifier(Modifier::BOLD)),
                Row::Item { keys, label, action } => {
                    let keys =
                        if keys.chars().count() > key_w { keys.chars().take(key_w - 1).collect::<String>() + "…" } else { keys.clone() };
                    let key_style = if *action { t.key_style() } else { Style::default().fg(t.fg).add_modifier(Modifier::BOLD) };
                    let mut line = Line::from(vec![
                        Span::raw("   "),
                        Span::styled(format!("{keys:<key_w$}"), key_style),
                        Span::raw("  "),
                        Span::raw(label.clone()),
                    ]);
                    if i == sel_row && n > 0 {
                        line = line.style(t.selected());
                    }
                    line
                }
            })
            .collect();
        if lines.is_empty() || n == 0 {
            f.render_widget(Paragraph::new(Line::styled(" no keys match", t.muted_style())), list);
        } else {
            f.render_widget(Paragraph::new(lines), list);
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(h: &mut Help, code: KeyCode) -> Outcome {
        h.on_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn sample() -> Help {
        let actions = vec![Action::new("x", "Delete the selected"), Action::new("r", "Refresh")];
        Help::new(rows("Docker", &actions, &[("c".into(), "Chats".into())]))
    }

    #[test]
    fn lists_screen_actions_then_app_keys() {
        let h = sample();
        assert_eq!(h.rows[0], Row::Header("Docker — actions".into()));
        assert!(matches!(&h.rows[1], Row::Item { keys, action: true, .. } if keys == "x"));
        assert!(h.rows.iter().any(|r| matches!(r, Row::Item { keys, .. } if keys == "gg")));
        assert!(h.rows.iter().any(|r| matches!(r, Row::Header(t) if t.starts_with("Leader"))));
        // no actions: no empty section
        let bare = rows("Home", &[], &[]);
        assert_eq!(bare[0], Row::Header("Motions".into()));
    }

    #[test]
    fn filter_keeps_headers_of_matches() {
        let h = sample();
        let v = visible(&h.rows, "refresh");
        assert_eq!(v, vec![Row::Header("Docker — actions".into()), Row::Item { keys: "r".into(), label: "Refresh".into(), action: true }]);
        assert!(visible(&h.rows, "zzzz").is_empty());
    }

    #[test]
    fn keys_move_filter_and_run() {
        let mut h = sample();
        assert_eq!(press(&mut h, KeyCode::Char('j')), Outcome::Stay);
        assert_eq!(h.selected, 1);
        assert_eq!(press(&mut h, KeyCode::Enter), Outcome::Run("r".into()));
        press(&mut h, KeyCode::Char('G'));
        assert_eq!(h.selected, h.items().len() - 1);
        press(&mut h, KeyCode::Char('g'));
        press(&mut h, KeyCode::Char('g'));
        assert_eq!(h.selected, 0);
        // an action key runs it directly
        assert_eq!(press(&mut h, KeyCode::Char('x')), Outcome::Run("x".into()));
        // `/` filters; the typed letters are not actions then
        press(&mut h, KeyCode::Char('/'));
        for c in "tab".chars() {
            assert_eq!(press(&mut h, KeyCode::Char(c)), Outcome::Stay);
        }
        assert!(h.items().iter().all(|r| matches!(r, Row::Item { keys, label, .. } if kit::matches(&format!("{keys} {label}"), "tab"))));
        press(&mut h, KeyCode::Esc);
        assert!(!h.typing && h.filter.is_empty());
        assert_eq!(press(&mut h, KeyCode::Esc), Outcome::Close);
    }

    #[test]
    fn a_filter_hides_empty_sections_and_clamps_the_selection() {
        let mut h = sample();
        press(&mut h, KeyCode::Char('G'));
        let last = h.selected;
        assert!(last > 3);
        // typing a filter starts from the first match again
        press(&mut h, KeyCode::Char('/'));
        for c in "refresh".chars() {
            press(&mut h, KeyCode::Char(c));
        }
        assert_eq!(h.selected, 0);
        assert_eq!(h.items().len(), 1);
        let shown = visible(&h.rows, &h.filter);
        assert_eq!(shown.iter().filter(|r| matches!(r, Row::Header(_))).count(), 1, "sections without a match are not listed");
        // moving past the end stays on the last match
        press(&mut h, KeyCode::Down);
        press(&mut h, KeyCode::Down);
        assert_eq!(h.selected, 0);
        // nothing matches: nothing selected, Enter does nothing
        press(&mut h, KeyCode::Char('z'));
        press(&mut h, KeyCode::Char('z'));
        assert!(h.items().is_empty());
        press(&mut h, KeyCode::Down);
        assert_eq!(h.selected, 0);
        assert_eq!(press(&mut h, KeyCode::Enter), Outcome::Stay, "Enter keeps the filter");
        assert!(!h.typing);
        assert_eq!(press(&mut h, KeyCode::Enter), Outcome::Stay, "no item to run");
        // a blank filter is no filter
        assert_eq!(visible(&h.rows, "  "), h.rows);
    }

    #[test]
    fn enter_runs_screen_actions_only() {
        let mut h = sample();
        // the first app key (after the two actions): Enter does not "run" a motion
        press(&mut h, KeyCode::Char('j'));
        press(&mut h, KeyCode::Char('j'));
        assert!(matches!(h.items().get(h.selected), Some(Row::Item { action: false, .. })));
        assert_eq!(press(&mut h, KeyCode::Enter), Outcome::Stay);
        // a letter that is an app key, not an action of this screen
        assert_eq!(press(&mut h, KeyCode::Char('z')), Outcome::Stay);
        assert_eq!(press(&mut h, KeyCode::Char('?')), Outcome::Close);
    }

    #[test]
    fn paste_goes_into_the_filter_only_while_it_is_typed() {
        let mut h = sample();
        h.on_paste("refresh");
        assert!(h.filter.is_empty(), "not typing: a paste is not a row of key presses");
        press(&mut h, KeyCode::Char('/'));
        h.on_paste("refresh");
        assert_eq!(h.filter, "refresh");
        assert_eq!(h.items().len(), 1);
    }

    #[test]
    fn draws_at_any_size() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        for (w, hgt) in [(120, 40), (40, 10), (10, 4)] {
            let mut term = Terminal::new(TestBackend::new(w, hgt)).unwrap();
            let mut h = sample();
            press(&mut h, KeyCode::Char('G'));
            term.draw(|f| {
                h.draw(f, f.area(), "Docker", Some("Containers on this machine"));
            })
            .unwrap();
            if w == 120 {
                let text: String = term.backend().buffer().content().iter().map(|c| c.symbol()).collect();
                assert!(text.contains("Keys · Docker"));
                assert!(text.contains("Leader"));
            }
        }
    }
}
