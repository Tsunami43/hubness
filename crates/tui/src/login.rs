//! Signing in to a hub with accounts. The session token of each hub is saved with the hub
//! (`crate::hubs`), so the TUI signs in once per hub; a 401 drops it and shows the screen
//! again.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use crate::ui::input::TextInput;
use crate::ui::theme::icon;
use crate::ui::{Theme, block, centered, hints, logo};

// ---------- the sign-in screen ----------

/// What a key did on the sign-in screen.
pub enum LoginEvent {
    None,
    Submit {
        username: String,
        password: String,
    },
    /// Ctrl+O: connect to another hub.
    OtherHub,
    Quit,
}

pub struct LoginForm {
    username: TextInput,
    password: TextInput,
    /// 0 — username, 1 — password.
    focus: usize,
    pub error: Option<String>,
    pub busy: bool,
}

impl LoginForm {
    pub fn new(username: Option<&str>, error: Option<String>) -> Self {
        let mut password = TextInput::default();
        password.secret = true;
        let username = TextInput::new(username.unwrap_or_default());
        let focus = usize::from(!username.is_empty());
        Self { username, password, focus, error, busy: false }
    }

    pub fn username(&self) -> String {
        self.username.value().trim().to_string()
    }

    /// A failed attempt: the message, and the password field ready for another try.
    pub fn failed(&mut self, error: String) {
        self.busy = false;
        self.error = Some(error);
        self.password.clear();
        self.focus = if self.username.is_empty() { 0 } else { 1 };
    }

    pub fn paste(&mut self, text: &str) {
        let line = text.lines().next().unwrap_or_default();
        if self.focus == 0 { self.username.paste(line) } else { self.password.paste(line) }
    }

    pub fn on_key(&mut self, k: KeyEvent) -> LoginEvent {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('q')) {
            return LoginEvent::Quit;
        }
        if ctrl && k.code == KeyCode::Char('o') {
            return LoginEvent::OtherHub;
        }
        if self.busy {
            return LoginEvent::None;
        }
        match k.code {
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Down | KeyCode::Up => self.focus = 1 - self.focus,
            KeyCode::Enter if self.focus == 0 => {
                if !self.username().is_empty() {
                    self.focus = 1;
                }
            }
            KeyCode::Enter => {
                let (username, password) = (self.username(), self.password.value());
                if username.is_empty() {
                    self.focus = 0;
                } else if !password.is_empty() {
                    self.busy = true;
                    self.error = None;
                    return LoginEvent::Submit { username, password };
                }
            }
            KeyCode::Esc => self.error = None,
            _ => {
                let input = if self.focus == 0 { &mut self.username } else { &mut self.password };
                input.on_key(k);
            }
        }
        LoginEvent::None
    }

    pub fn draw(&self, f: &mut Frame, url: &str) {
        let t = Theme::get();
        let area = f.area();
        f.render_widget(Clear, area);
        f.render_widget(Block::default().style(t.base()), area);
        let box_h = 13;
        let [top, form, _] = Layout::vertical([Constraint::Length(8), Constraint::Length(box_h), Constraint::Min(0)])
            .flex(ratatui::layout::Flex::Center)
            .areas(area);
        if area.height >= box_h + 8 {
            f.render_widget(logo(), top);
        }
        let r = centered(form, 66, box_h);
        let b = block(format!("{}  Sign in", icon("\u{f0342}", "")), true).style(Style::default().bg(t.bg_dark));
        let inner = b.inner(r);
        f.render_widget(b, r);
        let [hub, user, pass, msg, keys] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .areas(inner.inner(ratatui::layout::Margin { horizontal: 1, vertical: 0 }));
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{} ", icon("\u{f048b}", "hub")), t.accent_style()),
                Span::styled(url.to_string(), t.bold()),
                Span::styled("  asks for an account", t.muted_style()),
            ])),
            hub,
        );
        let title = |ic: &'static str, fallback: &'static str, name: &str| format!("{}  {name}", icon(ic, fallback)).trim().to_string();
        self.username.draw(f, user, block(title("\u{f0004}", "", "Username"), self.focus == 0), self.focus == 0 && !self.busy, "");
        self.password.draw(f, pass, block(title("\u{f033e}", "", "Password"), self.focus == 1), self.focus == 1 && !self.busy, "");
        let line = if self.busy {
            Line::styled("Signing in…", t.dim_style())
        } else if let Some(e) = &self.error {
            Line::styled(e.clone(), Style::default().fg(t.danger).add_modifier(Modifier::BOLD))
        } else {
            Line::styled("No account? Ask someone on this hub, or run `hubcli user add <name>` on its machine.", t.muted_style())
        };
        f.render_widget(Paragraph::new(line).wrap(Wrap { trim: true }), msg);
        f.render_widget(
            Paragraph::new(hints(&[("Enter", "sign in"), ("Tab", "next"), ("Ctrl+O", "other hub"), ("Ctrl+C", "quit")]))
                .alignment(Alignment::Center),
            keys,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_form_submits_both_fields() {
        let key = |c| KeyEvent::new(c, KeyModifiers::NONE);
        let mut form = LoginForm::new(Some("bob"), None);
        for c in "secret!!".chars() {
            form.on_key(key(KeyCode::Char(c)));
        }
        match form.on_key(key(KeyCode::Enter)) {
            LoginEvent::Submit { username, password } => assert_eq!((username.as_str(), password.as_str()), ("bob", "secret!!")),
            _ => panic!("expected a submit"),
        }
        assert!(form.busy);
        form.failed("Wrong username or password".into());
        assert!(!form.busy && form.password.is_empty());
    }
}
