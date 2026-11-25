//! The first screen without a saved hub (and `Space H` → Add a hub): the address of a hub,
//! plus the saved hubs to pick from.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::hubs::Hub;
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::theme::icon;
use crate::ui::{Theme, block, centered, hints, logo};

/// What a key did on the connect screen.
#[derive(Debug, PartialEq, Eq)]
pub enum ConnectEvent {
    None,
    /// Find a hub at the typed address.
    Connect(String),
    /// Open a saved hub.
    Open(String),
    /// Back to the hub the user came from.
    Back,
    Quit,
}

pub struct ConnectForm {
    input: TextInput,
    hubs: Vec<Hub>,
    /// `None` — the address box has the focus; `Some(i)` — the i-th saved hub.
    selected: Option<usize>,
    pub error: Option<String>,
    /// "Connecting to …" while a hub is being looked for.
    pub busy: Option<String>,
    /// The hub the user came from (Esc goes back to it).
    pub back: Option<String>,
    /// The address was filled in for the user: the first typed character replaces it.
    prefilled: bool,
}

impl ConnectForm {
    pub fn new(address: &str, hubs: Vec<Hub>, error: Option<String>, back: Option<String>) -> Self {
        Self { input: TextInput::new(address), hubs, selected: None, error, busy: None, back, prefilled: !address.is_empty() }
    }

    pub fn paste(&mut self, text: &str) {
        if self.selected.is_none() {
            self.input.paste(text.lines().next().unwrap_or_default().trim());
        }
    }

    pub fn on_key(&mut self, k: KeyEvent) -> ConnectEvent {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('q')) {
            return ConnectEvent::Quit;
        }
        if self.busy.is_some() {
            return ConnectEvent::None;
        }
        let n = self.hubs.len();
        match (self.selected, k.code) {
            (_, KeyCode::Esc) => {
                if self.back.is_some() {
                    return ConnectEvent::Back;
                }
                self.error = None;
                self.selected = None;
            }
            (None, KeyCode::Down | KeyCode::Tab) if n > 0 => self.selected = Some(0),
            (None, KeyCode::Enter) => {
                self.error = None;
                return ConnectEvent::Connect(self.input.value().trim().to_string());
            }
            (None, _) => {
                let typed = matches!(k.code, KeyCode::Char(_)) && !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL);
                if std::mem::take(&mut self.prefilled) && typed {
                    self.input.clear();
                }
                if self.input.on_key(k) == InputEvent::Changed {
                    self.error = None;
                }
            }
            (Some(i), KeyCode::Enter) => return ConnectEvent::Open(self.hubs[i].url.clone()),
            (Some(0), KeyCode::Up | KeyCode::BackTab) => self.selected = None,
            (Some(i), KeyCode::Up | KeyCode::BackTab | KeyCode::Char('k')) => self.selected = Some(i - 1),
            (Some(i), KeyCode::Down | KeyCode::Char('j')) => self.selected = Some((i + 1).min(n - 1)),
            (Some(_), KeyCode::Tab) => self.selected = None,
            _ => {}
        }
        ConnectEvent::None
    }

    pub fn draw(&self, f: &mut Frame) {
        let t = Theme::get();
        let area = f.area();
        f.render_widget(Clear, area);
        f.render_widget(Block::default().style(t.base()), area);
        let list_h = if self.hubs.is_empty() { 0 } else { self.hubs.len().min(6) as u16 + 2 };
        let box_h = 10 + list_h;
        let [top, form, _] = Layout::vertical([Constraint::Length(8), Constraint::Length(box_h), Constraint::Min(0)])
            .flex(ratatui::layout::Flex::Center)
            .areas(area);
        if area.height >= box_h + 8 {
            f.render_widget(logo(), top);
        }
        let r = centered(form, 64, box_h);
        let b = block(format!("{}  Connect to a hub", icon("\u{f048b}", "")), true).style(Style::default().bg(t.bg_dark));
        let inner = b.inner(r);
        f.render_widget(b, r);
        let [intro, address, msg, list, keys] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Length(list_h),
            Constraint::Length(1),
        ])
        .areas(inner.inner(Margin { horizontal: 1, vertical: 0 }));
        f.render_widget(
            Paragraph::new(Line::styled("A domain or an IP address; add :port when it isn't 8787.", t.muted_style()))
                .wrap(Wrap { trim: true }),
            intro,
        );
        let focus = self.selected.is_none() && self.busy.is_none();
        let title = format!("{}  Address", icon("\u{f0337}", "")).trim().to_string();
        self.input.draw(f, address, block(title, self.selected.is_none()), focus, "hub.example.com · 10.0.0.5:8787 · Enter: this machine");
        let line = if let Some(b) = &self.busy {
            Line::styled(b.clone(), t.dim_style())
        } else if let Some(e) = &self.error {
            Line::styled(e.clone(), Style::default().fg(t.danger).add_modifier(Modifier::BOLD))
        } else {
            Line::styled("https:// is tried first, then http:// on port 8787.", t.muted_style())
        };
        f.render_widget(Paragraph::new(line).wrap(Wrap { trim: true }), msg);
        if !self.hubs.is_empty() {
            let items: Vec<ListItem> = self
                .hubs
                .iter()
                .map(|h| {
                    let mut spans = vec![
                        Span::styled(format!(" {} ", icon("\u{f048b}", "-")), t.accent_style()),
                        Span::styled(h.url.clone(), t.bold()),
                    ];
                    if let Some(u) = &h.username {
                        spans.push(Span::styled(format!("  {u}"), t.muted_style()));
                    }
                    ListItem::new(Line::from(spans))
                })
                .collect();
            let mut state = ListState::default().with_selected(self.selected);
            f.render_stateful_widget(
                List::new(items).block(block("Saved hubs", self.selected.is_some())).highlight_style(t.selected()),
                list,
                &mut state,
            );
        }
        let mut pairs = vec![("Enter", "connect")];
        if !self.hubs.is_empty() {
            pairs.push(("Tab", "saved hubs"));
        }
        if self.back.is_some() {
            pairs.push(("Esc", "back"));
        }
        pairs.push(("Ctrl+C", "quit"));
        f.render_widget(Paragraph::new(hints(&pairs)).alignment(Alignment::Center), keys);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    #[test]
    fn typing_an_address_and_picking_a_saved_hub() {
        let hubs = vec![Hub { url: "http://a:1".into(), ..Default::default() }, Hub { url: "https://b".into(), ..Default::default() }];
        let mut form = ConnectForm::new("", hubs, None, None);
        for c in "hub.local".chars() {
            form.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(form.on_key(key(KeyCode::Enter)), ConnectEvent::Connect("hub.local".into()));
        form.on_key(key(KeyCode::Tab));
        form.on_key(key(KeyCode::Down));
        assert_eq!(form.on_key(key(KeyCode::Enter)), ConnectEvent::Open("https://b".into()));
        form.on_key(key(KeyCode::Up));
        form.on_key(key(KeyCode::Up));
        // back in the address box: letters type again
        form.on_key(key(KeyCode::Char('x')));
        assert_eq!(form.on_key(key(KeyCode::Enter)), ConnectEvent::Connect("hub.localx".into()));
        assert_eq!(form.on_key(key(KeyCode::Esc)), ConnectEvent::None, "nowhere to go back to");
        form.back = Some("http://a:1".into());
        assert_eq!(form.on_key(key(KeyCode::Esc)), ConnectEvent::Back);
    }
}
