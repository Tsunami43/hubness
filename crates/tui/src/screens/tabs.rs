//! A screen made of tabs (a project, a group): the tab bar, switching, and routing of
//! task / stream / modal answers to the tab that asked (through `Ctx::prefix`).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use serde_json::Value;

use crate::core::{Action, Ctx, Handled, Hit, Screen};
use crate::ui::Theme;

pub struct Tab {
    /// A stable name to pick the tab by (the agent kind on the Harness screen).
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
        // Alt+1…9 switch tabs; Alt+←/→ and Alt+Tab / Alt+Shift+Tab previous / next
        if k.modifiers.contains(KeyModifiers::ALT) {
            match k.code {
                KeyCode::Tab if !k.modifiers.contains(KeyModifiers::SHIFT) => {
                    self.select((self.active + 1) % self.tabs.len(), ctx);
                    return Handled::Yes;
                }
                // terminals send Shift+Tab as BackTab
                KeyCode::BackTab | KeyCode::Tab => {
                    self.select((self.active + self.tabs.len() - 1) % self.tabs.len(), ctx);
                    return Handled::Yes;
                }
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

    /// A click: on the tab bar (`bar:<i>`) or inside a tab (routed by its prefix).
    pub fn on_click(&mut self, id: &str, ctx: &mut Ctx) {
        if let Some(i) = id.strip_prefix("bar:").and_then(|i| i.parse().ok()) {
            return self.select(i, ctx);
        }
        if let Some((i, k)) = Self::route(id) {
            self.call(i, ctx, |s, c| s.on_click(&k, c));
        }
    }

    pub fn on_mouse(&mut self, m: crossterm::event::MouseEvent, ctx: &mut Ctx) -> Handled {
        let i = self.active;
        self.call(i, ctx, |s, c| s.on_mouse(m, c)).unwrap_or(Handled::No)
    }

    pub fn hints(&self) -> Vec<(String, String)> {
        self.tabs[self.active].screen.hints()
    }

    pub fn mode(&self) -> Option<&'static str> {
        self.tabs[self.active].screen.mode()
    }

    pub fn help(&self) -> Option<String> {
        self.tabs[self.active].screen.help()
    }

    pub fn names(&self) -> Vec<String> {
        self.tabs.iter().map(|t| t.label.clone()).collect()
    }

    pub fn captures_text(&self) -> bool {
        self.tabs[self.active].screen.captures_text()
    }

    /// The active tab's actions plus 1…9 to switch tabs.
    pub fn actions(&self) -> Vec<Action> {
        // tabs are switched with gt / gT, Alt+1…9 and Space t — not listed as screen actions
        self.tabs[self.active].screen.actions()
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
        // pills like Omarchy's waybar workspaces: the active one filled with the accent
        let mut spans = vec![Span::raw(" ")];
        for (i, tab) in self.tabs.iter().enumerate() {
            let x = bar.x + spans.iter().map(|s| s.content.chars().count() as u16).sum::<u16>();
            let w = tab.label.chars().count() as u16 + 4;
            ctx.click(Rect { x, y: bar.y, width: w.min((bar.x + bar.width).saturating_sub(x)), height: 1 }, Hit::Click(format!("bar:{i}")));
            if i == self.active {
                spans.push(Span::styled("\u{e0b6}", Style::default().fg(t.accent)));
                spans.push(Span::styled(
                    format!("{} {}", i + 1, tab.label),
                    Style::default().fg(t.bg).bg(t.accent).add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled("\u{e0b4}", Style::default().fg(t.accent)));
            } else {
                spans.push(Span::styled(format!(" {} ", i + 1), t.muted_style()));
                spans.push(Span::styled(format!("{} ", tab.label), t.dim_style()));
            }
            spans.push(Span::raw(" "));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), bar);
        let i = self.active;
        let saved = std::mem::replace(&mut *ctx.draw_prefix.borrow_mut(), Self::prefix(i));
        self.tabs[i].screen.draw(f, body, ctx);
        *ctx.draw_prefix.borrow_mut() = saved;
    }
}
