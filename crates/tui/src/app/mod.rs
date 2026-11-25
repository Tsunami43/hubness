//! The app shell: sidebar (sections and sessions), the active screen, the footer, the
//! leader (which-key) panel and modals. Mirrors the web app's layout and leader keys.

mod draw;

use std::collections::HashMap;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::core::{Action, AppEvent, Ctx, Handled, Hit, Modal, Request, Route, Screen, ScreenId, Shared, next_screen_id};
use crate::login::{LoginEvent, LoginForm};
use crate::screens;
use crate::ui::input::{InputEvent, TextInput};

/// A popup over the screen that is not a modal.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlay {
    None,
    /// `Space` / Ctrl+g and its groups.
    Leader(Menu),
}

/// Menus of the leader (which-key groups).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    Root,
    /// `Space a` (or `?`): the actions of the current screen.
    Actions,
    /// `Space w`: moving between panes.
    Windows,
    /// `Space t`: tabs.
    Tabs,
}

/// A which-key entry.
struct Entry {
    key: String,
    label: String,
    /// Opens another menu (shown with `+`).
    group: bool,
}

impl Entry {
    fn cmd(key: &str, label: &str) -> Self {
        Self { key: key.into(), label: label.into(), group: false }
    }
    fn group(key: &str, label: &str) -> Self {
        Self { key: key.into(), label: label.into(), group: true }
    }
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
    /// The sign-in screen of a hub with accounts (takes over the whole terminal).
    login: Option<LoginForm>,
    /// Who is signed in (`None` on a hub without accounts).
    user: Option<String>,
    pub quit: bool,
    /// Set with `quit` to open another hub instead of exiting.
    pub next: Option<Next>,
}

/// Where to go after this hub (`Space H`, Settings → Connection).
pub enum Next {
    /// A saved hub by its URL.
    Hub(String),
    /// The connect screen, to add a hub.
    Add,
}

const CACHE: usize = 12;
const APP: ScreenId = 0;

impl App {
    pub fn new(
        client: hubness_client::Client,
        tx: mpsc::UnboundedSender<AppEvent>,
        login: Option<LoginForm>,
        user: Option<String>,
    ) -> Self {
        let base_url = client.base().to_string();
        let ctx = Ctx {
            client,
            tx,
            screen: APP,
            requests: vec![],
            streams: HashMap::new(),
            status: None,
            prefix: String::new(),
            hits: Default::default(),
            draw_prefix: Default::default(),
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
            login,
            user,
            quit: false,
            next: None,
        };
        if app.login.is_none() {
            app.start();
        }
        app
    }

    /// Load the shared data, open the global stream and show the screen (again after a sign-in).
    fn start(&mut self) {
        self.refresh_shared();
        self.ctx.screen = APP;
        self.ctx.stream("global", "/stream");
        if self.screens.contains_key(&self.route) {
            self.with_screen(|s, ctx| s.on_show(ctx));
        } else {
            self.show(self.route.clone());
        }
    }

    /// Leave this hub: stop its live streams (the next hub gets a new app).
    pub fn shutdown(&mut self) {
        for (_, h) in self.ctx.streams.drain() {
            h.abort();
        }
    }

    /// Quit this hub and open another one (`None` — the connect screen).
    fn switch_hub(&mut self, url: Option<String>) {
        match url {
            Some(u) if crate::hubs::key(&u) == self.ctx.client.base() => self.ctx.info("Already on this hub"),
            Some(u) => {
                self.next = Some(Next::Hub(u));
                self.quit = true;
            }
            None => {
                self.next = Some(Next::Add);
                self.quit = true;
            }
        }
    }

    /// `Space H`: the saved hubs and "Add a hub…".
    fn open_hubs(&mut self) {
        let current = self.ctx.client.base().to_string();
        let mut items: Vec<(String, String)> = crate::hubs::Store::load()
            .hubs
            .into_iter()
            .map(|h| {
                let mut label = h.url.clone();
                if let Some(u) = &h.username {
                    label.push_str(&format!("  ({u})"));
                }
                if h.url == current {
                    label.push_str("  · current");
                }
                (format!("hub:{}", h.url), label)
            })
            .collect();
        items.push(("add".into(), "Add a hub…".into()));
        self.modal = Some((
            APP,
            Modal::Pick { title: "Switch hub".into(), items, filter: String::new(), selected: 0, key: "hubs".into() },
            TextInput::default(),
        ));
    }

    /// The session is gone (401) or the user signed out: back to the sign-in screen.
    fn sign_out(&mut self, message: Option<String>) {
        let url = self.ctx.client.base().to_string();
        crate::hubs::remember(&url, None, None);
        self.ctx.client.set_token(None);
        self.ctx.screen = APP;
        self.ctx.close_stream("global");
        self.modal = None;
        self.overlay = Overlay::None;
        let user = self.user.take().or_else(|| crate::hubs::stored(&url).1);
        self.login = Some(LoginForm::new(user.as_deref(), message));
    }

    fn on_login_key(&mut self, k: KeyEvent) {
        let Some(form) = &mut self.login else { return };
        match form.on_key(k) {
            LoginEvent::Quit => self.quit = true,
            LoginEvent::OtherHub => self.switch_hub(None),
            LoginEvent::Submit { username, password } => {
                let c = self.ctx.client.clone();
                self.ctx.screen = APP;
                self.ctx.task("login", async move { Ok(serde_json::to_value(c.login(&username, &password).await?)?) });
            }
            LoginEvent::None => {}
        }
    }

    fn on_login_result(&mut self, result: anyhow::Result<Value>) {
        let Some(form) = &mut self.login else { return };
        match result {
            Ok(v) => {
                let user = v["user"]["username"].as_str().map(String::from).unwrap_or_else(|| form.username());
                crate::hubs::remember(self.ctx.client.base(), v["token"].as_str(), Some(&user));
                self.user = Some(user);
                self.login = None;
                self.ctx.status = None;
                self.start();
            }
            Err(e) => {
                let msg = e.to_string();
                // "401: Wrong username or password" → the message
                let msg = msg.split_once(": ").filter(|(code, _)| code.parse::<u16>().is_ok()).map_or(msg.clone(), |(_, m)| m.to_string());
                form.failed(msg);
            }
        }
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
                Request::SwitchHub(url) => self.switch_hub(url),
            }
        }
    }

    pub fn handle(&mut self, ev: AppEvent) {
        if self.login.is_some() {
            match ev {
                AppEvent::Input(Event::Key(k)) if k.kind != KeyEventKind::Release => self.on_login_key(k),
                AppEvent::Input(Event::Paste(text)) => {
                    if let Some(form) = &mut self.login {
                        form.paste(&text);
                    }
                }
                AppEvent::Task { screen: APP, key, result } if key == "login" => self.on_login_result(result),
                // answers to requests made before the session ended
                _ => {}
            }
            self.ctx.client.take_unauthorized();
            return;
        }
        self.handle_app(ev);
        if self.login.is_none() && self.ctx.client.take_unauthorized() {
            // no user: accounts were turned on while the TUI ran (the first one was just added)
            let msg =
                if self.user.is_some() { "Your session has ended: sign in again" } else { "The hub asks for an account now: sign in" };
            self.sign_out(Some(msg.into()));
        }
    }

    fn handle_app(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Task { screen: APP, key, result } if key == "logout" => {
                let _ = result;
                self.sign_out(Some("Signed out".into()));
            }
            AppEvent::Input(Event::Key(k)) if k.kind != KeyEventKind::Release => self.on_key(k),
            AppEvent::Input(Event::Paste(text)) => {
                if let Some((_, Modal::Input { .. }, input)) = &mut self.modal {
                    input.paste(&text);
                } else {
                    self.with_screen(|s, ctx| s.on_paste(&text, ctx));
                }
            }
            AppEvent::Input(Event::Mouse(m)) => self.on_mouse(m),
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
        if let Overlay::Leader(menu) = self.overlay {
            self.overlay = Overlay::None;
            // Backspace goes up a level, Esc closes
            if k.code == KeyCode::Backspace && menu != Menu::Root {
                self.overlay = Overlay::Leader(Menu::Root);
                return;
            }
            return self.on_leader_key(menu, k);
        }
        if ctrl && k.code == KeyCode::Char('q') {
            self.quit = true;
            return;
        }
        // Ctrl+g opens the leader everywhere (also while typing)
        if ctrl && matches!(k.code, KeyCode::Char('g') | KeyCode::Char(' ')) {
            self.reset_pending();
            self.overlay = Overlay::Leader(Menu::Root);
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
                "^wh" | "^w^h" | "^wk" | "^w^k" => self.focus_prev(),
                "^wl" | "^w^l" | "^wj" | "^w^j" | "^ww" => self.send_key(KeyCode::Tab, none, 1),
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
            (KeyCode::Char('h'), true) => self.focus_prev(),
            (KeyCode::Char('l'), true) => self.send_key(KeyCode::Tab, none, 1),
            (KeyCode::Char(' '), false) => self.overlay = Overlay::Leader(Menu::Root),
            (KeyCode::Char('?'), false) => self.overlay = Overlay::Leader(Menu::Actions),
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

    /// Focus the previous pane (Shift+Tab); screens with two panes may only know Tab.
    fn focus_prev(&mut self) {
        let ev = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        if matches!(self.with_screen(|s, ctx| s.on_key(ev, ctx)).unwrap_or(Handled::No), Handled::No) {
            self.send_key(KeyCode::Tab, KeyModifiers::NONE, 1);
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

    /// A region of the app's own chrome (sidebar, status line, which-key).
    fn hit(&self, area: Rect, hit: Hit) {
        self.ctx.hits.borrow_mut().push((area, APP, hit));
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        let pos = Position { x: m.column, y: m.row };
        let up = match m.kind {
            MouseEventKind::ScrollUp => Some(true),
            MouseEventKind::ScrollDown => Some(false),
            _ => None,
        };
        if self.modal.is_some() {
            // the wheel moves a picker; clicks stay inside the popup
            if let Some(up) = up {
                self.on_key(KeyEvent::new(if up { KeyCode::Up } else { KeyCode::Down }, KeyModifiers::NONE));
            }
            return;
        }
        let hit = self.ctx.hits.borrow().iter().rev().find(|(r, _, _)| r.contains(pos)).map(|(_, id, h)| (*id, h.clone()));
        if m.kind == MouseEventKind::Down(MouseButton::Left) {
            if let Some((APP, Hit::Key(k))) = hit
                && self.overlay != Overlay::None
            {
                if k.code == KeyCode::Null {
                    return;
                }
                // a which-key entry: as if its key was pressed
                return self.on_key(k);
            }
            self.overlay = Overlay::None;
            self.reset_pending();
            match hit {
                Some((_, Hit::Go(route))) => self.go(route),
                // status line: `? more`, `␣ leader` (work in INSERT too)
                Some((APP, Hit::Key(k))) => match k.code {
                    KeyCode::Char('?') => self.overlay = Overlay::Leader(Menu::Actions),
                    KeyCode::Char(' ') => self.overlay = Overlay::Leader(Menu::Root),
                    _ => self.on_key(k),
                },
                Some((id, Hit::Key(k))) => {
                    self.with_screen_id(id, |s, ctx| s.on_key(k, ctx));
                }
                // a status-line hint: the screen's own action (also editor hints such as `:w`)
                Some((APP, Hit::Action(a))) => {
                    self.with_screen(|s, ctx| s.on_action(&a, ctx));
                }
                Some((id, Hit::Action(a))) => {
                    self.with_screen_id(id, |s, ctx| s.on_action(&a, ctx));
                }
                Some((id, Hit::Click(c))) => {
                    self.with_screen_id(id, |s, ctx| s.on_click(&c, ctx));
                }
                None => {
                    self.with_screen(|s, ctx| s.on_mouse(m, ctx));
                }
            }
            return;
        }
        if let Some(up) = up {
            if matches!(hit, Some((APP, Hit::Go(_)))) {
                return; // the sidebar does not scroll
            }
            if self.with_screen(|s, ctx| s.on_mouse(m, ctx)).is_none_or(|h| matches!(h, Handled::No)) {
                self.send_key(if up { KeyCode::Up } else { KeyCode::Down }, KeyModifiers::NONE, 2);
            }
            return;
        }
        // drags and releases: an editor selects text with them
        self.with_screen(|s, ctx| s.on_mouse(m, ctx));
    }

    fn screen_actions(&self) -> Vec<Action> {
        self.screens.get(&self.route).map(|s| s.screen.actions()).unwrap_or_default()
    }

    /// Entries of a leader menu (LazyVim-style which-key; groups are marked `+`).
    fn leader_entries(&self, menu: Menu) -> Vec<Entry> {
        match menu {
            Menu::Root => vec![
                Entry::cmd("␣", "Find a session"),
                Entry::cmd("c", "Chats"),
                Entry::cmd("p", "Projects"),
                Entry::cmd("s", "System"),
                Entry::cmd("h", "Harness (agents)"),
                Entry::cmd(",", "Settings"),
                Entry::group("a", "actions"),
                Entry::group("w", "windows"),
                Entry::group("t", "tabs"),
                Entry::cmd("e", "Sidebar"),
                Entry::cmd("r", "Reload"),
                Entry::cmd("H", "Switch hub"),
                Entry::cmd("L", "Log out"),
                Entry::cmd("q", "Quit"),
            ],
            Menu::Windows => vec![
                Entry::cmd("h", "Pane left"),
                Entry::cmd("j", "Pane below"),
                Entry::cmd("k", "Pane above"),
                Entry::cmd("l", "Pane right"),
                Entry::cmd("w", "Next pane"),
            ],
            Menu::Tabs => {
                let mut e = vec![Entry::cmd("n", "Next tab"), Entry::cmd("p", "Previous tab")];
                let names = self.screens.get(&self.route).map(|s| s.screen.tab_names()).unwrap_or_default();
                for (i, name) in names.into_iter().enumerate().take(9) {
                    e.push(Entry { key: (i + 1).to_string(), label: name, group: false });
                }
                e
            }
            Menu::Actions => {
                self.screen_actions().into_iter().map(|a| Entry { key: a.key.to_string(), label: a.label, group: false }).collect()
            }
        }
    }

    fn on_leader_key(&mut self, menu: Menu, k: KeyEvent) {
        let KeyCode::Char(c) = k.code else { return };
        let key = c.to_string();
        match menu {
            Menu::Root => match key.as_str() {
                " " => self.open_jump(),
                "c" => self.go(Route::Home),
                "p" => self.go(Route::Projects),
                "s" => self.go(Route::System(None)),
                "h" => self.go(Route::Harness(None)),
                "," => self.go(Route::Settings(None)),
                "a" => self.overlay = Overlay::Leader(Menu::Actions),
                "w" => self.overlay = Overlay::Leader(Menu::Windows),
                "t" => self.overlay = Overlay::Leader(Menu::Tabs),
                "e" => self.sidebar = !self.sidebar,
                "r" => {
                    self.refresh_shared();
                    self.with_screen(|s, ctx| s.on_show(ctx));
                }
                "H" => self.open_hubs(),
                "L" => self.log_out(),
                "q" => self.quit = true,
                _ => {}
            },
            Menu::Windows => match key.as_str() {
                "h" | "k" => self.focus_prev(),
                "l" | "j" | "w" => self.send_key(KeyCode::Tab, KeyModifiers::NONE, 1),
                _ => {}
            },
            Menu::Tabs => match key.as_str() {
                "n" => self.next_tab(true, 1),
                "p" => self.next_tab(false, 1),
                d if c.is_ascii_digit() && d != "0" => self.send_key(KeyCode::Char(c), KeyModifiers::ALT, 1),
                _ => {}
            },
            Menu::Actions => self.run_action(&key),
        }
        self.apply_requests();
    }

    /// End the session on the hub, then show the sign-in screen.
    fn log_out(&mut self) {
        if self.user.is_none() {
            self.ctx.info("This hub has no accounts: there is nothing to log out of");
            return;
        }
        let c = self.ctx.client.clone();
        self.ctx.screen = APP;
        self.ctx.task("logout", async move { c.logout().await.map(|_| Value::Null) });
    }

    /// Run a screen action by its key.
    fn run_action(&mut self, key: &str) {
        if let Some(a) = self.screen_actions().into_iter().find(|a| a.key == key) {
            self.with_screen(|s, ctx| s.on_action(a.key, ctx));
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
            ("cmd:hub", "Switch hub"),
            ("cmd:logout", "Log out"),
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
            "cmd:hub" => self.open_hubs(),
            "cmd:logout" => self.log_out(),
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
                    KeyCode::Home => selected = 0,
                    KeyCode::End => selected = visible.len().saturating_sub(1),
                    KeyCode::PageDown => selected = (selected + 10).min(visible.len().saturating_sub(1)),
                    KeyCode::PageUp => selected = selected.saturating_sub(10),
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
                } else if key == "hubs"
                    && let Some(id) = value.as_str()
                {
                    let url = id.strip_prefix("hub:").map(String::from);
                    self.switch_hub(url);
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
        Ok(mut child) => {
            // reap it, or every opened link stays a zombie until the TUI exits
            std::thread::spawn(move || child.wait());
            ctx.info(format!("opened {url}"));
        }
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
