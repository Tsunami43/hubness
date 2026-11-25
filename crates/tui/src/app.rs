//! The app shell: sidebar (sections and sessions), the active screen, the footer, the
//! leader (which-key) panel and modals. Mirrors the web app's layout and leader keys.

use std::collections::HashMap;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::core::{Action, AppEvent, Ctx, Handled, Modal, Request, Route, Screen, ScreenId, Section, Shared, next_screen_id};
use crate::screens;
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::theme::icon;
use crate::ui::{Theme, block, centered};

/// A popup over the screen that is not a modal.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlay {
    None,
    /// `Space` / Ctrl+g: the global leader keys.
    Leader,
    /// `?`: the keys of the current screen.
    Help,
}

struct Slot {
    id: ScreenId,
    screen: Box<dyn Screen>,
}

pub struct App {
    ctx: Ctx,
    route: Route,
    history: Vec<Route>,
    screens: HashMap<Route, Slot>,
    /// Least recently used last; screens beyond the cache size are dropped.
    lru: Vec<Route>,
    overlay: Overlay,
    /// The first key of a two-key sequence (`g`, Ctrl+w as `^w`).
    pending: String,
    /// A count typed before a motion (`5j`).
    count: Option<usize>,
    modal: Option<(ScreenId, Modal, TextInput)>,
    sidebar: bool,
    pub quit: bool,
}

const CACHE: usize = 12;
const APP: ScreenId = 0;

impl App {
    pub fn new(client: hubness_client::Client, tx: mpsc::UnboundedSender<AppEvent>) -> Self {
        let base_url = client.base().to_string();
        let ctx = Ctx {
            client,
            tx,
            screen: APP,
            requests: vec![],
            streams: HashMap::new(),
            status: None,
            prefix: String::new(),
            shared: Shared { base_url, ..Default::default() },
        };
        let mut app = Self {
            ctx,
            route: Route::Home,
            history: vec![],
            screens: HashMap::new(),
            lru: vec![],
            overlay: Overlay::None,
            pending: String::new(),
            count: None,
            modal: None,
            sidebar: true,
            quit: false,
        };
        app.refresh_shared();
        app.ctx.stream("global", "/stream");
        app.show(Route::Home);
        app
    }

    /// Hub-wide data shared by the screens.
    fn refresh_shared(&mut self) {
        self.ctx.screen = APP;
        self.ctx.get("shared:agents", "/agents");
        self.ctx.get("shared:settings", "/settings");
        self.ctx.get("shared:sessions", "/sessions?all=1");
        self.ctx.get("shared:projects", "/projects");
        self.ctx.get("shared:groups", "/groups");
    }

    fn on_shared(&mut self, key: &str, result: anyhow::Result<Value>) {
        let Ok(v) = result else { return };
        let list = |v: Value| v.as_array().cloned().unwrap_or_default();
        match key {
            "shared:agents" => self.ctx.shared.agents = v,
            "shared:settings" => self.ctx.shared.settings = v,
            "shared:sessions" => self.ctx.shared.sessions = list(v),
            "shared:projects" => self.ctx.shared.projects = list(v),
            "shared:groups" => self.ctx.shared.groups = list(v),
            _ => {}
        }
    }

    /// The global stream keeps the session list live.
    fn on_global(&mut self, msg: &Value) {
        let sessions = &mut self.ctx.shared.sessions;
        match msg["type"].as_str() {
            Some("session") => {
                let s = msg["session"].clone();
                let id = s["id"].clone();
                sessions.retain(|x| x["id"] != id);
                if s["archivedAt"].is_null() {
                    sessions.insert(0, s);
                    sessions.sort_by(|a, b| b["updatedAt"].as_str().cmp(&a["updatedAt"].as_str()));
                }
            }
            Some("session-deleted") => sessions.retain(|x| x["id"] != msg["id"]),
            _ => {}
        }
    }

    fn show(&mut self, route: Route) {
        if !self.screens.contains_key(&route) {
            let id = next_screen_id();
            let screen = screens::make(&route);
            self.screens.insert(route.clone(), Slot { id, screen });
        }
        self.lru.retain(|r| r != &route);
        self.lru.insert(0, route.clone());
        while self.lru.len() > CACHE {
            if let Some(old) = self.lru.pop()
                && let Some(slot) = self.screens.remove(&old)
            {
                self.drop_streams(slot.id);
            }
        }
        self.route = route;
        self.with_screen(|s, ctx| s.on_show(ctx));
    }

    fn drop_streams(&mut self, id: ScreenId) {
        let keys: Vec<_> = self.ctx.streams.keys().filter(|(s, _)| *s == id).cloned().collect();
        for k in keys {
            if let Some(h) = self.ctx.streams.remove(&k) {
                h.abort();
            }
        }
    }

    fn go(&mut self, route: Route) {
        if route != self.route {
            self.history.push(self.route.clone());
            if self.history.len() > 50 {
                self.history.remove(0);
            }
            self.show(route);
        }
    }

    /// Run `f` on the active screen with the context pointed at it, then apply its requests.
    fn with_screen<R>(&mut self, f: impl FnOnce(&mut dyn Screen, &mut Ctx) -> R) -> Option<R> {
        let route = self.route.clone();
        let slot = self.screens.get_mut(&route)?;
        self.ctx.screen = slot.id;
        let r = f(slot.screen.as_mut(), &mut self.ctx);
        self.apply_requests();
        Some(r)
    }

    fn with_screen_id<R>(&mut self, id: ScreenId, f: impl FnOnce(&mut dyn Screen, &mut Ctx) -> R) -> Option<R> {
        let slot = self.screens.values_mut().find(|s| s.id == id)?;
        self.ctx.screen = id;
        let r = f(slot.screen.as_mut(), &mut self.ctx);
        self.apply_requests();
        Some(r)
    }

    fn apply_requests(&mut self) {
        let requests = std::mem::take(&mut self.ctx.requests);
        let from = self.ctx.screen;
        for r in requests {
            match r {
                Request::Go(route) => self.go(route),
                Request::Back => {
                    if let Some(prev) = self.history.pop() {
                        self.show(prev);
                    }
                }
                Request::Modal(m) => {
                    let input = match &m {
                        Modal::Input { value, secret, multiline, .. } => {
                            let mut i = TextInput::new(value);
                            i.secret = *secret;
                            i.multiline = *multiline;
                            i
                        }
                        _ => TextInput::default(),
                    };
                    self.modal = Some((from, m, input));
                }
                Request::OpenUrl(url) => open_url(&url, &mut self.ctx),
            }
        }
    }

    pub fn handle(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Input(Event::Key(k)) if k.kind != KeyEventKind::Release => self.on_key(k),
            AppEvent::Input(Event::Paste(text)) => {
                if let Some((_, Modal::Input { .. }, input)) = &mut self.modal {
                    input.paste(&text);
                } else {
                    self.with_screen(|s, ctx| s.on_paste(&text, ctx));
                }
            }
            AppEvent::Input(_) => {}
            AppEvent::Task { screen: APP, key, result } => self.on_shared(&key, result),
            AppEvent::Task { screen, key, result } => {
                self.with_screen_id(screen, |s, ctx| s.on_task(&key, result, ctx));
            }
            AppEvent::Stream { screen: APP, msg, .. } => self.on_global(&msg),
            AppEvent::Stream { screen, key, msg } => {
                self.with_screen_id(screen, |s, ctx| s.on_stream(&key, msg, ctx));
            }
            AppEvent::StreamEnded { screen: APP, .. } => {
                // the hub restarted or the network dropped: reconnect and reload
                self.ctx.screen = APP;
                self.ctx.stream("global", "/stream");
                self.refresh_shared();
            }
            AppEvent::StreamEnded { screen, key, error } => {
                self.with_screen_id(screen, |s, ctx| s.on_stream(&key, serde_json::json!({ "type": "__ended", "error": error }), ctx));
            }
            AppEvent::Global(msg) => self.on_global(&msg),
        }
    }

    /// Keys: popups first, then the overlays (leader / which-key), then Vim NORMAL-mode
    /// motions and globals, then the screen. See crates/tui/DESIGN.md.
    fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if self.modal.is_some() {
            return self.on_modal_key(k);
        }
        match self.overlay {
            Overlay::Leader => {
                self.overlay = Overlay::None;
                return self.on_leader_key(k);
            }
            Overlay::Help => {
                self.overlay = Overlay::None;
                // a key from the which-key list runs its action
                if let KeyCode::Char(c) = k.code
                    && let Some(a) = self.screen_actions().into_iter().find(|a| a.key == c.to_string())
                {
                    self.with_screen(|s, ctx| s.on_action(a.key, ctx));
                }
                return;
            }
            Overlay::None => {}
        }
        if ctrl && k.code == KeyCode::Char('q') {
            self.quit = true;
            return;
        }
        // Ctrl+g opens the leader everywhere (also while typing)
        if ctrl && matches!(k.code, KeyCode::Char('g') | KeyCode::Char(' ')) {
            self.reset_pending();
            self.overlay = Overlay::Leader;
            return;
        }
        let typing = self.typing();
        if typing {
            // INSERT: everything goes to the screen (Esc leaves the input there)
            self.with_screen(|s, ctx| s.on_key(k, ctx));
            return;
        }
        self.on_normal_key(k);
        self.apply_requests();
    }

    fn typing(&self) -> bool {
        self.screens.get(&self.route).is_some_and(|s| s.screen.captures_text())
    }

    fn reset_pending(&mut self) {
        self.pending.clear();
        self.count = None;
    }

    /// Send a key to the screen `n` times.
    fn send_key(&mut self, code: KeyCode, mods: KeyModifiers, n: usize) {
        let ev = KeyEvent::new(code, mods);
        for _ in 0..n.clamp(1, 500) {
            self.with_screen(|s, ctx| s.on_key(ev, ctx));
        }
    }

    fn on_normal_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let none = KeyModifiers::NONE;
        let n = self.count.unwrap_or(1);
        // two-key sequences: gg, gt, gT, Ctrl+w h/l
        if !self.pending.is_empty() {
            let seq = format!("{}{}", self.pending, key_name(&k));
            self.reset_pending();
            match seq.as_str() {
                "gg" => self.send_key(KeyCode::Home, none, 1),
                "gt" => self.next_tab(true, n),
                "gT" => self.next_tab(false, n),
                "^wh" | "^w^h" => self.send_key(KeyCode::BackTab, KeyModifiers::SHIFT, 1),
                "^wl" | "^w^l" | "^ww" => self.send_key(KeyCode::Tab, none, 1),
                _ => {}
            }
            return;
        }
        match (k.code, ctrl) {
            // counts: 5j, 10G
            (KeyCode::Char(c @ '0'..='9'), false) if k.modifiers.is_empty() && (c != '0' || self.count.is_some()) => {
                self.count = Some(self.count.unwrap_or(0) * 10 + (c as usize - '0' as usize));
                return;
            }
            (KeyCode::Char('g'), false) => self.pending = "g".into(),
            (KeyCode::Char('w'), true) => self.pending = "^w".into(),
            (KeyCode::Char('j'), false) | (KeyCode::Down, false) => self.send_key(KeyCode::Down, none, n),
            (KeyCode::Char('k'), false) | (KeyCode::Up, false) => self.send_key(KeyCode::Up, none, n),
            (KeyCode::Char('h'), false) => self.send_key(KeyCode::Left, none, n),
            (KeyCode::Char('l'), false) => self.send_key(KeyCode::Right, none, n),
            (KeyCode::Char('G'), false) => self.send_key(KeyCode::End, none, 1),
            (KeyCode::Char('d'), true) => self.send_key(KeyCode::PageDown, none, n),
            (KeyCode::Char('u'), true) => self.send_key(KeyCode::PageUp, none, n),
            (KeyCode::Char('f'), true) => self.send_key(KeyCode::PageDown, none, 2 * n),
            (KeyCode::Char('b'), true) => self.send_key(KeyCode::PageUp, none, 2 * n),
            (KeyCode::Char('h'), true) => self.send_key(KeyCode::BackTab, KeyModifiers::SHIFT, 1),
            (KeyCode::Char('l'), true) => self.send_key(KeyCode::Tab, none, 1),
            (KeyCode::Char(' '), false) => self.overlay = Overlay::Leader,
            (KeyCode::Char('?'), false) => self.overlay = Overlay::Help,
            (KeyCode::Char(':'), false) => self.open_palette(),
            (KeyCode::Char('q'), false) => {
                // a screen closes its own popup first
                if matches!(self.with_screen(|s, ctx| s.on_key(k, ctx)).unwrap_or(Handled::No), Handled::No) {
                    self.back_or_quit();
                }
            }
            (KeyCode::Char('c'), true) => self.quit = true,
            _ => {
                // the screen's own keys: Enter, Esc, Tab, `/`, `i`, actions…
                let handled = self.with_screen(|s, ctx| s.on_key(k, ctx)).unwrap_or(Handled::No);
                if matches!(handled, Handled::No) {
                    match k.code {
                        KeyCode::Esc => self.back_or_quit_soft(),
                        KeyCode::Left if k.modifiers.contains(KeyModifiers::ALT) => self.ctx.back(),
                        KeyCode::Char(c) if !ctrl => {
                            let key = c.to_string();
                            if let Some(a) = self.screen_actions().into_iter().find(|a| a.key == key) {
                                self.with_screen(|s, ctx| s.on_action(a.key, ctx));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        if k.code != KeyCode::Char('g') && !(ctrl && k.code == KeyCode::Char('w')) {
            self.count = None;
        }
    }

    /// Next / previous tab of a tabbed screen (Alt+→ / Alt+← for it).
    fn next_tab(&mut self, forward: bool, n: usize) {
        let code = if forward { KeyCode::Right } else { KeyCode::Left };
        self.send_key(code, KeyModifiers::ALT, n);
    }

    fn back_or_quit(&mut self) {
        if self.history.is_empty() {
            self.quit = true;
        } else {
            self.ctx.back();
        }
    }

    /// Esc in NORMAL: back a screen (never quits).
    fn back_or_quit_soft(&mut self) {
        if !self.history.is_empty() {
            self.ctx.back();
        }
    }

    fn screen_actions(&self) -> Vec<Action> {
        self.screens.get(&self.route).map(|s| s.screen.actions()).unwrap_or_default()
    }

    /// Global leader keys (Space …), as LazyVim's which-key.
    fn leader_actions(&self) -> Vec<(&'static str, &'static str, Option<Route>)> {
        vec![
            (" ", "Jump to a session", None),
            ("c", "Chats", Some(Route::Home)),
            ("p", "Projects", Some(Route::Projects)),
            ("s", "System", Some(Route::System(None))),
            ("h", "Harness (agents)", Some(Route::Harness(None))),
            (",", "Settings", Some(Route::Settings(None))),
            ("e", "Toggle sidebar", None),
            ("1-9", "Tab 1…9", None),
            ("r", "Reload", None),
            ("q", "Quit", None),
        ]
    }

    fn on_leader_key(&mut self, k: KeyEvent) {
        let KeyCode::Char(c) = k.code else { return };
        if let Some(d) = c.to_digit(10).filter(|d| *d > 0) {
            self.send_key(KeyCode::Char(char::from_digit(d, 10).unwrap_or('1')), KeyModifiers::ALT, 1);
            return;
        }
        let key = c.to_string();
        let route = self.leader_actions().into_iter().find(|(kk, _, _)| *kk == key).and_then(|(_, _, r)| r);
        match (key.as_str(), route) {
            (_, Some(r)) => self.go(r),
            (" ", _) => self.open_jump(),
            ("e", _) => self.sidebar = !self.sidebar,
            ("r", _) => {
                self.refresh_shared();
                self.with_screen(|s, ctx| s.on_show(ctx));
            }
            ("q", _) => self.quit = true,
            // a screen action reached through the leader
            _ => {
                if let Some(a) = self.screen_actions().into_iter().find(|a| a.key == key) {
                    self.with_screen(|s, ctx| s.on_action(a.key, ctx));
                }
            }
        }
    }

    fn open_jump(&mut self) {
        let items = self
            .ctx
            .shared
            .sessions
            .iter()
            .map(|s| (s["id"].as_str().unwrap_or_default().to_string(), session_label(s, &self.ctx.shared)))
            .collect();
        self.modal = Some((
            APP,
            Modal::Pick { title: "Jump to a session".into(), items, filter: String::new(), selected: 0, key: "jump".into() },
            TextInput::default(),
        ));
    }

    /// `:` — every screen action and global command, fuzzy-filtered.
    fn open_palette(&mut self) {
        let mut items: Vec<(String, String)> =
            self.screen_actions().into_iter().map(|a| (format!("action:{}", a.key), format!("{}   ({})", a.label, a.key))).collect();
        for (id, label) in [
            ("go:chats", "Go to chats"),
            ("go:projects", "Go to projects"),
            ("go:system", "Go to system"),
            ("go:harness", "Go to harness (agents)"),
            ("go:settings", "Go to settings"),
            ("cmd:jump", "Jump to a session"),
            ("cmd:sidebar", "Toggle sidebar"),
            ("cmd:reload", "Reload"),
            ("cmd:back", "Back"),
            ("cmd:quit", "Quit  (:q)"),
        ] {
            items.push((id.into(), label.into()));
        }
        self.modal = Some((
            APP,
            Modal::Pick { title: ":".into(), items, filter: String::new(), selected: 0, key: "palette".into() },
            TextInput::default(),
        ));
    }

    fn run_palette(&mut self, id: &str) {
        match id {
            "go:chats" => self.go(Route::Home),
            "go:projects" => self.go(Route::Projects),
            "go:system" => self.go(Route::System(None)),
            "go:harness" => self.go(Route::Harness(None)),
            "go:settings" => self.go(Route::Settings(None)),
            "cmd:jump" => self.open_jump(),
            "cmd:sidebar" => self.sidebar = !self.sidebar,
            "cmd:reload" => {
                self.refresh_shared();
                self.with_screen(|s, ctx| s.on_show(ctx));
            }
            "cmd:back" => self.ctx.back(),
            "cmd:quit" => self.quit = true,
            other => {
                if let Some(key) = other.strip_prefix("action:")
                    && let Some(a) = self.screen_actions().into_iter().find(|a| a.key == key)
                {
                    self.with_screen(|s, ctx| s.on_action(a.key, ctx));
                }
            }
        }
        self.apply_requests();
    }

    fn on_modal_key(&mut self, k: KeyEvent) {
        let Some((owner, modal, mut input)) = self.modal.take() else { return };
        let mut answer: Option<(String, Value)> = None;
        let mut keep = None;
        match modal {
            Modal::Input { title, value, key, secret, multiline } => match input.on_key(k) {
                InputEvent::Submit => answer = Some((key, Value::String(input.value()))),
                InputEvent::Cancel => answer = Some((key, Value::Null)),
                _ => keep = Some(Modal::Input { title, value, key, secret, multiline }),
            },
            Modal::Confirm { title, message, key, danger } => match k.code {
                KeyCode::Enter | KeyCode::Char('y') => answer = Some((key, Value::Bool(true))),
                KeyCode::Esc | KeyCode::Char('n') => answer = Some((key, Value::Bool(false))),
                _ => keep = Some(Modal::Confirm { title, message, key, danger }),
            },
            Modal::Message { title, text } => match k.code {
                KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') => {}
                _ => keep = Some(Modal::Message { title, text }),
            },
            Modal::Pick { title, items, mut filter, mut selected, key } => {
                let visible = filtered(&items, &filter);
                match k.code {
                    KeyCode::Esc => answer = Some((key.clone(), Value::Null)),
                    KeyCode::Enter => {
                        answer = Some((key.clone(), visible.get(selected).map(|(id, _)| Value::String(id.clone())).unwrap_or(Value::Null)))
                    }
                    KeyCode::Down => selected = (selected + 1).min(visible.len().saturating_sub(1)),
                    KeyCode::Up => selected = selected.saturating_sub(1),
                    KeyCode::Char('n') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        selected = (selected + 1).min(visible.len().saturating_sub(1))
                    }
                    KeyCode::Char('p') if k.modifiers.contains(KeyModifiers::CONTROL) => selected = selected.saturating_sub(1),
                    KeyCode::Backspace => {
                        filter.pop();
                        selected = 0;
                    }
                    KeyCode::Char(c) => {
                        filter.push(c);
                        selected = 0;
                    }
                    _ => {}
                }
                if answer.is_none() {
                    keep = Some(Modal::Pick { title, items, filter, selected, key });
                }
            }
        }
        if let Some(m) = keep {
            self.modal = Some((owner, m, input));
        }
        if let Some((key, value)) = answer {
            if owner == APP {
                if key == "palette"
                    && let Some(id) = value.as_str()
                {
                    let id = id.to_string();
                    self.run_palette(&id);
                } else if key == "jump"
                    && let Some(id) = value.as_str()
                {
                    let route = self.session_route(id);
                    self.go(route);
                }
            } else {
                self.with_screen_id(owner, |s, ctx| s.on_task(&key, Ok(value), ctx));
            }
        }
    }

    /// Where a session lives: its project, group, or the chats.
    fn session_route(&self, id: &str) -> Route {
        let s = self.ctx.shared.sessions.iter().find(|s| s["id"] == id);
        match s.map(|s| (s["projectId"].as_str(), s["groupId"].as_str())) {
            Some((Some(p), _)) => Route::ProjectSession { project: p.into(), session: id.into() },
            Some((None, Some(g))) => Route::GroupSession { group: g.into(), session: id.into() },
            _ => Route::Chat(id.into()),
        }
    }

    // ---------- drawing ----------

    // ---------- drawing ----------

    pub fn draw(&mut self, f: &mut Frame) {
        let t = Theme::get();
        // the theme's background and text everywhere
        f.render_widget(Block::default().style(t.base()), f.area());
        let [main, status] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(f.area());
        let content = if self.sidebar && main.width > 90 {
            let [side, content] = Layout::horizontal([Constraint::Length(30), Constraint::Min(40)]).areas(main);
            self.draw_sidebar(f, side);
            content
        } else {
            main
        };
        let route = self.route.clone();
        if let Some(slot) = self.screens.get_mut(&route) {
            self.ctx.screen = slot.id;
            slot.screen.draw(f, content, &self.ctx);
        }
        self.draw_status(f, status);
        match self.overlay {
            Overlay::Leader => self.draw_which_key(f, main, true),
            Overlay::Help => self.draw_which_key(f, main, false),
            Overlay::None => {}
        }
        if let Some((_, modal, input)) = &self.modal {
            draw_modal(f, main, modal, input);
        }
    }

    /// Mode, where you are, the message, agents at work, the hub — like Neovim's lualine.
    fn draw_status(&self, f: &mut Frame, area: Rect) {
        let t = Theme::get();
        let (mode, color) = if self.modal.is_some() {
            if matches!(self.modal, Some((_, Modal::Pick { ref key, .. }, _)) if key == "palette") {
                ("COMMAND", t.warn)
            } else {
                ("PROMPT", t.magenta)
            }
        } else if self.overlay == Overlay::Leader {
            ("LEADER", t.magenta)
        } else if self.typing() {
            ("INSERT", t.ok)
        } else {
            ("NORMAL", t.accent)
        };
        let pill = Style::default().fg(t.bg).bg(color).add_modifier(Modifier::BOLD);
        let mut left =
            vec![Span::styled(format!(" {mode} "), pill), Span::styled(icon("\u{e0b4}", ""), Style::default().fg(color).bg(t.bg_dark))];
        let crumb = self.breadcrumb();
        left.push(Span::styled(format!(" {crumb} "), Style::default().fg(t.fg).bg(t.bg_dark)));
        left.push(Span::styled(icon("\u{e0b4}", ""), Style::default().fg(t.bg_dark).bg(t.bg)));
        if !self.pending.is_empty() || self.count.is_some() {
            left.push(Span::styled(
                format!("  {}{}", self.count.map(|c| c.to_string()).unwrap_or_default(), self.pending.replace('^', "Ctrl+")),
                t.accent_style(),
            ));
        }
        let msg = match &self.ctx.status {
            Some((m, true)) => Span::styled(format!("  {m}"), Style::default().fg(t.danger)),
            Some((m, false)) => Span::styled(format!("  {m}"), t.dim_style()),
            None => Span::styled("  ? keys · : commands · Space leader", t.muted_style()),
        };
        left.push(msg);
        let sessions = &self.ctx.shared.sessions;
        let running = sessions.iter().filter(|s| s["status"] == "running").count();
        let waiting = sessions.iter().filter(|s| s["status"] == "waiting").count();
        let host = self.ctx.shared.base_url.trim_start_matches("http://").trim_start_matches("https://").to_string();
        let mut right = vec![];
        if running > 0 {
            right.push(Span::styled(format!(" ● {running} "), Style::default().fg(t.ok)));
        }
        if waiting > 0 {
            right.push(Span::styled(format!(" ? {waiting} "), Style::default().fg(t.warn).add_modifier(Modifier::BOLD)));
        }
        right.push(Span::styled(icon("\u{e0b6}", ""), Style::default().fg(t.bg_dark).bg(t.bg)));
        right.push(Span::styled(format!(" {} {host} ", icon("󰒋", "hub")), Style::default().fg(t.dim).bg(t.bg_dark)));
        right
            .push(Span::styled(format!(" {} ", icon("󰣇", "hubness")), Style::default().fg(t.bg).bg(t.accent).add_modifier(Modifier::BOLD)));
        let rw: u16 = right.iter().map(|s| s.content.chars().count() as u16).sum();
        let [l, r] = Layout::horizontal([Constraint::Min(10), Constraint::Length(rw)]).areas(area);
        f.render_widget(Paragraph::new(Line::from(left)).style(Style::default().bg(t.bg)), l);
        f.render_widget(Paragraph::new(Line::from(right)).style(Style::default().bg(t.bg)), r);
    }

    fn breadcrumb(&self) -> String {
        let shared = &self.ctx.shared;
        let name =
            |list: &Vec<Value>, id: &str| list.iter().find(|x| x["id"] == id).and_then(|x| x["name"].as_str()).unwrap_or("…").to_string();
        let title = self.screens.get(&self.route).map(|s| s.screen.title()).unwrap_or_default();
        match &self.route {
            Route::Home => format!("{} Chats", icon("󰭹", "")),
            Route::Chat(_) => format!("{} Chats › {title}", icon("󰭹", "")),
            Route::Projects => format!("{} Projects", icon("\u{f401}", "")),
            Route::Project(id) => format!("{} Projects › {}", icon("\u{f401}", ""), name(&shared.projects, id)),
            Route::ProjectSession { project, .. } => format!("{} {} › {title}", icon("\u{f401}", ""), name(&shared.projects, project)),
            Route::Group(id) => format!("{} Groups › {}", icon("󰉋", ""), name(&shared.groups, id)),
            Route::GroupSession { group, .. } => format!("{} {} › {title}", icon("󰉋", ""), name(&shared.groups, group)),
            Route::System(_) => format!("{} System", icon("󰍹", "")),
            Route::Harness(_) => format!("{} Harness", icon("󰚩", "")),
            Route::Settings(_) => format!("{} Settings", icon("\u{f013}", "")),
        }
        .trim()
        .to_string()
    }

    fn draw_sidebar(&self, f: &mut Frame, area: Rect) {
        let t = Theme::get();
        f.render_widget(Block::default().style(Style::default().bg(t.bg_dark)), area);
        let section = self.route.section();
        let mut lines: Vec<Line> = vec![
            Line::from(vec![
                Span::styled(format!(" {} ", icon("󰣇", "")), Style::default().fg(t.accent)),
                Span::styled("hubness", Style::default().fg(t.fg).add_modifier(Modifier::BOLD)),
            ]),
            Line::raw(""),
        ];
        for (s, label, key, ic) in [
            (Section::Chats, "Chats", "c", icon("󰭹", "·")),
            (Section::Projects, "Projects", "p", icon("\u{f401}", "·")),
            (Section::System, "System", "s", icon("󰍹", "·")),
            (Section::Harness, "Harness", "h", icon("󰚩", "·")),
            (Section::Settings, "Settings", ",", icon("\u{f013}", "·")),
        ] {
            let on = s == section;
            let bar = if on { Span::styled("▎", Style::default().fg(t.accent)) } else { Span::raw(" ") };
            let style = if on { Style::default().fg(t.accent).add_modifier(Modifier::BOLD) } else { Style::default().fg(t.dim) };
            lines.push(Line::from(vec![
                bar,
                Span::styled(format!("{ic}  {label}"), style),
                Span::styled(format!("  ␣{key}"), t.muted_style()),
            ]));
        }
        lines.push(Line::raw(""));
        lines.push(Line::styled(" SESSIONS", t.muted_style().add_modifier(Modifier::BOLD)));
        let current = match &self.route {
            Route::Chat(id) | Route::ProjectSession { session: id, .. } | Route::GroupSession { session: id, .. } => Some(id.as_str()),
            _ => None,
        };
        let room = area.height.saturating_sub(lines.len() as u16 + 1) as usize;
        let width = area.width.saturating_sub(4) as usize;
        for s in self.ctx.shared.sessions.iter().take(room) {
            let (mark, color) = t.status(s["status"].as_str().unwrap_or("idle"));
            let on = current == s["id"].as_str();
            let title: String = s["title"].as_str().unwrap_or_default().chars().take(width).collect();
            let style = if on { t.selected() } else { Style::default().fg(t.fg) };
            lines.push(Line::from(vec![Span::styled(format!(" {mark} "), Style::default().fg(color)), Span::styled(title, style)]));
        }
        f.render_widget(
            Paragraph::new(lines)
                .block(Block::default().borders(Borders::RIGHT).border_style(t.line_style()).style(Style::default().bg(t.bg_dark))),
            area,
        );
    }

    /// which-key: the leader's global keys (`Space`) or the screen's keys (`?`).
    fn draw_which_key(&self, f: &mut Frame, area: Rect, leader: bool) {
        let t = Theme::get();
        let entries: Vec<(String, String)> = if leader {
            self.leader_actions()
                .into_iter()
                .map(|(k, l, _)| (if k == " " { "␣".to_string() } else { k.to_string() }, l.to_string()))
                .collect()
        } else {
            let mut e: Vec<(String, String)> = self.screen_actions().into_iter().map(|a| (a.key.to_string(), a.label)).collect();
            for (k, l) in [
                ("j k", "down / up"),
                ("h l", "left / right"),
                ("gg G", "top / bottom"),
                ("^d ^u", "half page"),
                ("^h ^l", "focus pane"),
                ("gt gT", "next / prev tab"),
                ("/", "filter"),
                (":", "commands"),
                ("q", "back"),
            ] {
                e.push((k.into(), l.into()));
            }
            e
        };
        let cols = ((area.width.saturating_sub(4)) / 32).max(1) as usize;
        let rows = entries.len().div_ceil(cols);
        let h = (rows as u16 + 2).min(area.height);
        let rect = Rect { x: area.x + 1, y: area.y + area.height.saturating_sub(h), width: area.width.saturating_sub(2), height: h };
        f.render_widget(Clear, rect);
        let title = if leader {
            format!("{}  leader", icon("󰌌", ""))
        } else {
            format!("{}  {}", icon("󰋖", "?"), self.screens.get(&self.route).map(|s| s.screen.title()).unwrap_or_default())
        };
        let b = block(title, true).style(Style::default().bg(t.bg_dark));
        let inner = b.inner(rect);
        f.render_widget(b, rect);
        let col_w = inner.width / cols as u16;
        for (i, (k, l)) in entries.iter().enumerate() {
            let (c, r) = (i / rows, i % rows);
            let cell = Rect { x: inner.x + c as u16 * col_w, y: inner.y + r as u16, width: col_w, height: 1 };
            let line = Line::from(vec![
                Span::styled(format!("{k:>5}"), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
                Span::styled(" → ", t.muted_style()),
                Span::styled(l.clone(), Style::default().fg(t.fg)),
            ]);
            f.render_widget(Paragraph::new(line), cell);
        }
    }
}

/// Items whose label contains every word of the filter (case-insensitive).
fn filtered<'a>(items: &'a [(String, String)], filter: &str) -> Vec<&'a (String, String)> {
    let words: Vec<String> = filter.to_lowercase().split_whitespace().map(String::from).collect();
    items.iter().filter(|(_, l)| words.iter().all(|w| l.to_lowercase().contains(w))).collect()
}

pub fn session_label(s: &Value, shared: &Shared) -> String {
    let scope = match (s["projectId"].as_str(), s["groupId"].as_str()) {
        (Some(p), _) => shared.projects.iter().find(|x| x["id"] == p).and_then(|x| x["name"].as_str()).unwrap_or("project").to_string(),
        (None, Some(g)) => shared.groups.iter().find(|x| x["id"] == g).and_then(|x| x["name"].as_str()).unwrap_or("group").to_string(),
        _ => "chat".into(),
    };
    format!("{}  · {} · {}", s["title"].as_str().unwrap_or_default(), scope, s["agent"]["kind"].as_str().unwrap_or_default())
}

fn draw_modal(f: &mut Frame, area: Rect, modal: &Modal, input: &TextInput) {
    let t = Theme::get();
    match modal {
        Modal::Input { title, multiline, .. } => {
            let h = if *multiline { 10 } else { 3 };
            let r = centered(area, 70, h);
            f.render_widget(Clear, r);
            input.draw(f, r, block(title.clone(), true), true, "");
        }
        Modal::Confirm { title, message, danger, .. } => {
            // wrapped lines count too (62 columns inside, with slack for word wrapping)
            let lines = message.lines().map(|l| l.chars().count().div_ceil(56).max(1)).sum::<usize>() as u16 + 4;
            let r = centered(area, 64, lines.max(5));
            f.render_widget(Clear, r);
            let b = block(title.clone(), true);
            let inner = b.inner(r);
            f.render_widget(b, r);
            let mut text: Vec<Line> = message.lines().map(|l| Line::raw(l.to_string())).collect();
            text.push(Line::raw(""));
            let yes = if *danger {
                Style::default().fg(t.danger).add_modifier(Modifier::BOLD)
            } else {
                t.accent_style().add_modifier(Modifier::BOLD)
            };
            text.push(Line::from(vec![Span::styled("[y] confirm", yes), Span::styled("   [n / Esc] cancel", t.muted_style())]));
            f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
        }
        Modal::Message { title, text } => {
            let r = centered(area, 80, (text.lines().count() as u16 + 3).min(area.height));
            f.render_widget(Clear, r);
            f.render_widget(Paragraph::new(text.clone()).wrap(Wrap { trim: false }).block(block(title.clone(), true)), r);
        }
        Modal::Pick { title, items, filter, selected, .. } => {
            let visible = filtered(items, filter);
            let h = (visible.len() as u16 + 4).clamp(6, area.height.saturating_sub(2));
            let r = centered(area, 80, h);
            f.render_widget(Clear, r);
            let b = block(title.clone(), true);
            let inner = b.inner(r);
            f.render_widget(b, r);
            let [q, list] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
            f.render_widget(Paragraph::new(Line::from(vec![Span::styled("› ", t.accent_style()), Span::raw(filter.clone())])), q);
            let rows = list.height as usize;
            let start = selected.saturating_sub(rows.saturating_sub(1));
            let items: Vec<ListItem> = visible
                .iter()
                .enumerate()
                .skip(start)
                .take(rows)
                .map(|(i, (_, label))| {
                    ListItem::new(Line::styled(format!(" {label}"), if i == *selected { t.selected() } else { Style::default() }))
                })
                .collect();
            f.render_widget(List::new(items), list);
        }
    }
    let _ = Direction::Vertical;
}

/// Open a URL in the local browser (xdg-open / open).
fn open_url(url: &str, ctx: &mut Ctx) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    match std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => ctx.info(format!("opened {url}")),
        Err(_) => ctx.info(format!("open in a browser: {url}")),
    }
}

/// A key as typed in a sequence: `g`, `T`, Ctrl+w → `^w`.
fn key_name(k: &KeyEvent) -> String {
    match k.code {
        KeyCode::Char(c) if k.modifiers.contains(KeyModifiers::CONTROL) => format!("^{c}"),
        KeyCode::Char(c) => c.to_string(),
        _ => String::new(),
    }
}
