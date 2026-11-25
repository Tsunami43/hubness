//! A screen made of tabs (a project, a group): the tab bar, switching, and routing of
//! task / stream / modal answers to the tab that asked (through `Ctx::prefix`).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use serde_json::Value;

use crate::core::{Action, Ctx, Handled, Screen};
use crate::ui::Theme;

#[allow(dead_code)] // `id` is for tabs that open other tabs (select_id)
pub struct Tab {
    pub id: String,
    pub label: String,
    pub screen: Box<dyn Screen>,
    shown: bool,
}

impl Tab {
    pub fn new(id: impl Into<String>, label: impl Into<String>, screen: Box<dyn Screen>) -> Self {
        Self { id: id.into(), label: label.into(), screen, shown: false }
    }
}

pub struct Tabs {
    pub tabs: Vec<Tab>,
    pub active: usize,
}

const DIGITS: [&str; 9] = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];

impl Tabs {
    pub fn new(tabs: Vec<Tab>) -> Self {
        Self { tabs, active: 0 }
    }

    fn prefix(i: usize) -> String {
        format!("tab{i}:")
    }

    /// Call the tab `i` with the context prefixed for it.
    pub fn call<R>(&mut self, i: usize, ctx: &mut Ctx, f: impl FnOnce(&mut dyn Screen, &mut Ctx) -> R) -> Option<R> {
        let tab = self.tabs.get_mut(i)?;
        let saved = std::mem::replace(&mut ctx.prefix, Self::prefix(i));
        let r = f(tab.screen.as_mut(), ctx);
        ctx.prefix = saved;
        Some(r)
    }

    pub fn select(&mut self, i: usize, ctx: &mut Ctx) {
        if i >= self.tabs.len() {
            return;
        }
        self.active = i;
        self.tabs[i].shown = true;
        self.call(i, ctx, |s, c| s.on_show(c));
    }

    #[allow(dead_code)]
    pub fn select_id(&mut self, id: &str, ctx: &mut Ctx) {
        if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
            self.select(i, ctx);
        }
    }

    /// Route a prefixed key back to its tab: (tab index, the tab's own key).
    fn route(key: &str) -> Option<(usize, String)> {
        let rest = key.strip_prefix("tab")?;
        let (i, k) = rest.split_once(':')?;
        Some((i.parse().ok()?, k.to_string()))
    }

    pub fn on_show(&mut self, ctx: &mut Ctx) {
        let i = self.active;
        self.tabs[i].shown = true;
        self.call(i, ctx, |s, c| s.on_show(c));
    }

    pub fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) -> Option<anyhow::Result<Value>> {
        match Self::route(key) {
            Some((i, k)) => {
                self.call(i, ctx, |s, c| s.on_task(&k, result, c));
                None
            }
            None => Some(result),
        }
    }

    pub fn on_stream(&mut self, key: &str, msg: Value, ctx: &mut Ctx) -> Option<Value> {
        match Self::route(key) {
            Some((i, k)) => {
                self.call(i, ctx, |s, c| s.on_stream(&k, msg, c));
                None
            }
            None => Some(msg),
        }
    }

    pub fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        // Alt+1…9 switch tabs; Alt+←/→ previous / next
        if k.modifiers.contains(KeyModifiers::ALT) {
            match k.code {
                KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                    self.select(c as usize - '1' as usize, ctx);
                    return Handled::Yes;
                }
                KeyCode::Right => {
                    self.select((self.active + 1) % self.tabs.len(), ctx);
                    return Handled::Yes;
                }
                KeyCode::Left => {
                    self.select((self.active + self.tabs.len() - 1) % self.tabs.len(), ctx);
                    return Handled::Yes;
                }
                _ => {}
            }
        }
        let i = self.active;
        self.call(i, ctx, |s, c| s.on_key(k, c)).unwrap_or(Handled::No)
    }

    pub fn on_paste(&mut self, text: &str, ctx: &mut Ctx) {
        let i = self.active;
        self.call(i, ctx, |s, c| s.on_paste(text, c));
    }

    pub fn captures_text(&self) -> bool {
        self.tabs[self.active].screen.captures_text()
    }

    /// The active tab's actions plus 1…9 to switch tabs.
    pub fn actions(&self) -> Vec<Action> {
        let mut a = self.tabs[self.active].screen.actions();
        for (i, t) in self.tabs.iter().enumerate().take(9) {
            a.push(Action::new(DIGITS[i], format!("Tab: {}", t.label)));
        }
        a
    }

    pub fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        if let Some(i) = DIGITS.iter().position(|d| *d == key) {
            return self.select(i, ctx);
        }
        let i = self.active;
        self.call(i, ctx, |s, c| s.on_action(key, c));
    }

    /// The tab bar on the first line of `area`; the rest is the active tab.
    pub fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = Theme::get();
        let [bar, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
        let mut spans = vec![];
        for (i, tab) in self.tabs.iter().enumerate() {
            let style = if i == self.active { t.key_style() } else { t.muted_style() };
            spans.push(Span::styled(format!(" {} {} ", i + 1, tab.label), style));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled("  Alt+1…9 / Alt+←→", t.muted_style()));
        f.render_widget(Paragraph::new(Line::from(spans)), bar);
        let i = self.active;
        self.tabs[i].screen.draw(f, body, ctx);
    }
}
