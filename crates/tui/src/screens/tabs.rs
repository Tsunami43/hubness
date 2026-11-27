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
        let i = self.active;
        if !k.modifiers.contains(KeyModifiers::ALT) {
            return self.call(i, ctx, |s, c| s.on_key(k, c)).unwrap_or(Handled::No);
        }
        let n = self.tabs.len();
        // Alt+Tab / Alt+Shift+Tab always switch (terminals send Shift+Tab as BackTab)
        match k.code {
            KeyCode::Tab if !k.modifiers.contains(KeyModifiers::SHIFT) => {
                self.select((i + 1) % n, ctx);
                return Handled::Yes;
            }
            KeyCode::BackTab | KeyCode::Tab => {
                self.select((i + n - 1) % n, ctx);
                return Handled::Yes;
            }
            _ => {}
        }
        // Alt+←/→ and Alt+1…9 switch too — unless the tab is typing: an input moves by
        // words with Alt+←/→ and a shell gets Alt+digits, so the tab sees them first
        let target = match k.code {
            KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => Some(c as usize - '1' as usize),
            KeyCode::Right => Some((i + 1) % n),
            KeyCode::Left => Some((i + n - 1) % n),
            _ => None,
        };
        let Some(target) = target else {
            return self.call(i, ctx, |s, c| s.on_key(k, c)).unwrap_or(Handled::No);
        };
        if self.captures_text() && matches!(self.call(i, ctx, |s, c| s.on_key(k, c)), Some(Handled::Yes)) {
            return Handled::Yes;
        }
        self.select(target, ctx);
        Handled::Yes
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::test_ctx;

    /// A tab that records the keys it gets; `typing` makes it take every key, like an input.
    struct Probe {
        typing: bool,
        keys: std::rc::Rc<std::cell::RefCell<Vec<KeyCode>>>,
    }

    impl Screen for Probe {
        fn title(&self) -> String {
            "probe".into()
        }
        fn draw(&mut self, _f: &mut Frame, _area: Rect, _ctx: &Ctx) {}
        fn on_key(&mut self, key: KeyEvent, _ctx: &mut Ctx) -> Handled {
            self.keys.borrow_mut().push(key.code);
            if self.typing { Handled::Yes } else { Handled::No }
        }
        fn captures_text(&self) -> bool {
            self.typing
        }
    }

    fn tabs(typing: [bool; 3]) -> (Tabs, std::rc::Rc<std::cell::RefCell<Vec<KeyCode>>>) {
        let keys = std::rc::Rc::new(std::cell::RefCell::new(vec![]));
        let list = typing
            .iter()
            .enumerate()
            .map(|(i, t)| {
                Tab::new(format!("t{i}"), format!("Tab {i}"), Box::new(Probe { typing: *t, keys: keys.clone() }) as Box<dyn Screen>)
            })
            .collect();
        (Tabs::new(list), keys)
    }

    fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    #[test]
    fn prefixed_keys_find_their_tab() {
        assert_eq!(Tabs::route("tab0:list"), Some((0, "list".to_string())));
        assert_eq!(Tabs::route("tab12:key"), Some((12, "key".to_string())));
        assert_eq!(Tabs::route("tab3:delete:a:b"), Some((3, "delete:a:b".to_string())), "the tab's own key may have colons");
        assert_eq!(Tabs::route("tab2:"), Some((2, String::new())));
        // not a tab's key: the container's own
        for own in ["list", "tab:x", "tabx:y", "tab1", "table:1", ""] {
            assert_eq!(Tabs::route(own), None, "{own}");
        }
        assert_eq!(Tabs::prefix(4), "tab4:");
    }

    #[test]
    fn alt_keys_switch_tabs() {
        let mut ctx = test_ctx();
        let (mut t, keys) = tabs([false, false, false]);
        t.on_key(alt(KeyCode::Right), &mut ctx);
        assert_eq!(t.active, 1);
        t.on_key(alt(KeyCode::Left), &mut ctx);
        t.on_key(alt(KeyCode::Left), &mut ctx);
        assert_eq!(t.active, 2, "wraps around");
        t.on_key(alt(KeyCode::Char('1')), &mut ctx);
        assert_eq!(t.active, 0);
        t.on_key(alt(KeyCode::Char('9')), &mut ctx);
        assert_eq!(t.active, 0, "no such tab");
        t.on_key(alt(KeyCode::Tab), &mut ctx);
        assert_eq!(t.active, 1);
        t.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::ALT | KeyModifiers::SHIFT), &mut ctx);
        assert_eq!(t.active, 0);
        assert!(keys.borrow().is_empty(), "a tab that isn't typing never sees the switching keys");
        // other keys go to the active tab
        assert!(matches!(t.on_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE), &mut ctx), Handled::No));
        assert!(matches!(t.on_key(alt(KeyCode::Char('b')), &mut ctx), Handled::No));
        assert_eq!(*keys.borrow(), [KeyCode::Char('x'), KeyCode::Char('b')]);
    }

    #[test]
    fn a_typing_tab_gets_alt_arrows_and_digits_first() {
        let mut ctx = test_ctx();
        let (mut t, keys) = tabs([true, false, false]);
        // word left in an input, Alt+2 in a shell: not a tab switch
        assert!(matches!(t.on_key(alt(KeyCode::Left), &mut ctx), Handled::Yes));
        assert!(matches!(t.on_key(alt(KeyCode::Char('2')), &mut ctx), Handled::Yes));
        assert_eq!(t.active, 0);
        assert_eq!(*keys.borrow(), [KeyCode::Left, KeyCode::Char('2')]);
        // Alt+Tab still leaves a typing tab
        t.on_key(alt(KeyCode::Tab), &mut ctx);
        assert_eq!(t.active, 1);
        assert_eq!(keys.borrow().len(), 2);
    }
}
