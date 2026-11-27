//! The TUI's runtime model: screens, the context they act through, and the events that
//! flow back to them.
//!
//! A screen never blocks: it starts API calls (`Ctx::task`) and live streams
//! (`Ctx::stream`) tagged with a key; results come back to the same screen instance through
//! `Screen::on_task` / `Screen::on_stream`. Tasks and streams of a closed screen are dropped.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};

use crossterm::event::KeyEvent;
use hubness_client::Client;
use ratatui::Frame;
use ratatui::layout::Rect;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Where the app can go (mirrors the web app's routes).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Route {
    /// Chats: the session list and the new-chat composer.
    Home,
    Chat(String),
    Projects,
    Project(String),
    ProjectSession {
        project: String,
        session: String,
    },
    Group(String),
    GroupSession {
        group: String,
        session: String,
    },
    System(Option<String>),
    Harness(Option<String>),
    Settings(Option<String>),
}

impl Route {
    /// The section of the sidebar the route belongs to.
    pub fn section(&self) -> Section {
        match self {
            Route::Home | Route::Chat(_) => Section::Chats,
            Route::Projects | Route::Project(_) | Route::ProjectSession { .. } => Section::Projects,
            Route::Group(_) | Route::GroupSession { .. } => Section::Projects,
            Route::System(_) => Section::System,
            Route::Harness(_) => Section::Harness,
            Route::Settings(_) => Section::Settings,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Chats,
    Projects,
    System,
    Harness,
    Settings,
}

/// A key a screen offers, shown in the which-key panel and the footer.
#[derive(Debug, Clone)]
pub struct Action {
    pub key: &'static str,
    pub label: String,
}

impl Action {
    pub fn new(key: &'static str, label: impl Into<String>) -> Self {
        Self { key, label: label.into() }
    }
}

/// What a mouse click on a region does.
#[derive(Clone, Debug)]
pub enum Hit {
    Go(Route),
    /// A screen action by its key (as from `actions()`).
    Action(String),
    /// A key sent to the screen.
    Key(KeyEvent),
    /// A screen-defined target: `Screen::on_click(id)` (list rows: `"<list>:<index>"`).
    Click(String),
}

/// What a key press did.
pub enum Handled {
    /// The screen used it.
    Yes,
    /// Not the screen's key: the app may use it (navigation, leader, quit).
    No,
}

pub type ScreenId = u64;

/// One screen of the app.
/// Screens live on the app loop's task and never cross threads, so they need not be `Send`
/// (editor state from edtui holds an `Rc`).
pub trait Screen {
    fn title(&self) -> String;
    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx);
    fn on_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Handled;
    /// A task this screen started has finished.
    fn on_task(&mut self, _key: &str, _result: anyhow::Result<Value>, _ctx: &mut Ctx) {}
    /// A message of a live stream this screen opened.
    fn on_stream(&mut self, _key: &str, _msg: Value, _ctx: &mut Ctx) {}
    /// Screen-specific actions for the which-key panel.
    fn actions(&self) -> Vec<Action> {
        vec![]
    }
    /// The screen is typing text: single letters are not shortcuts.
    fn captures_text(&self) -> bool {
        false
    }
    /// Called when the screen becomes visible (first time and again later: refresh here).
    fn on_show(&mut self, _ctx: &mut Ctx) {}
    /// One of `actions()` chosen from the which-key panel (or its key pressed directly).
    fn on_action(&mut self, _key: &str, _ctx: &mut Ctx) {}
    /// Pasted text (bracketed paste).
    fn on_paste(&mut self, _text: &str, _ctx: &mut Ctx) {}
    /// A click on a region the screen registered with `Ctx::click`.
    fn on_click(&mut self, _id: &str, _ctx: &mut Ctx) {}
    /// A mouse event no registered region took (wheel, clicks inside an editor).
    fn on_mouse(&mut self, _m: crossterm::event::MouseEvent, _ctx: &mut Ctx) -> Handled {
        Handled::No
    }
    /// The mode for the status line while the screen takes the keys (`INSERT` by default).
    fn mode(&self) -> Option<&'static str> {
        None
    }
    /// Key hints for the status line; by default the screen's actions.
    fn hints(&self) -> Vec<(String, String)> {
        self.actions().into_iter().map(|a| (a.key.to_string(), a.label)).collect()
    }
    /// Names of the screen's tabs (for the `Space t` menu); empty when it has none.
    fn tab_names(&self) -> Vec<String> {
        vec![]
    }
    /// A short plain-words explanation of what the screen shows, added to the which-key title.
    fn help(&self) -> Option<String> {
        None
    }
}

/// Events delivered to the app loop.
pub enum AppEvent {
    Input(crossterm::event::Event),
    Task {
        screen: ScreenId,
        key: String,
        result: anyhow::Result<Value>,
    },
    Stream {
        screen: ScreenId,
        key: String,
        msg: Value,
    },
    StreamEnded {
        screen: ScreenId,
        key: String,
        error: Option<String>,
    },
    /// Global session changes (the sidebar list, statuses).
    Global(Value),
}

/// A modal the app shows over the screen; its answer goes back to the screen as a task
/// result with the given key (`Value::String` for inputs, `Value::Bool` for confirms,
/// the chosen item's id for pickers; `Value::Null` when cancelled).
pub enum Modal {
    Input { title: String, value: String, key: String, secret: bool, multiline: bool },
    Confirm { title: String, message: String, key: String, danger: bool },
    Pick { title: String, items: Vec<(String, String)>, filter: String, selected: usize, key: String },
    Message { title: String, text: String },
}

/// Navigation and UI requests a screen makes.
pub enum Request {
    Go(Route),
    Back,
    Modal(Modal),
    /// Open a URL in the local browser (preview, provider links).
    OpenUrl(String),
    /// Leave this hub for a saved one (`None` — the connect screen, to add one).
    SwitchHub(Option<String>),
    /// End the session on the hub and show the sign-in screen.
    LogOut,
}

/// What a screen acts through.
pub struct Ctx {
    pub client: Client,
    pub(crate) tx: mpsc::UnboundedSender<AppEvent>,
    pub(crate) screen: ScreenId,
    pub(crate) requests: Vec<Request>,
    pub(crate) streams: HashMap<(ScreenId, String), JoinHandle<()>>,
    pub status: Option<(String, bool)>,
    /// Prepended to task / stream / modal keys while a container (project, group) calls one
    /// of its tabs, so answers find their way back to that tab. Screens never set it.
    pub(crate) prefix: String,
    /// Clickable regions of the frame being drawn (the last one on top wins).
    pub(crate) hits: std::cell::RefCell<Vec<(Rect, ScreenId, Hit)>>,
    /// The tab prefix while a tab draws (click ids are routed back like task keys).
    pub(crate) draw_prefix: std::cell::RefCell<String>,
    /// Hub-wide data the screens share (agents, settings), refreshed by the app.
    pub shared: Shared,
}

/// Data several screens need, loaded once and kept fresh by the app.
#[derive(Default, Clone)]
pub struct Shared {
    /// `GET /api/agents`.
    pub agents: Value,
    /// `GET /api/settings`.
    pub settings: Value,
    /// Every active session (`GET /api/sessions?all=1`), kept live by the global stream.
    pub sessions: Vec<Value>,
    /// `GET /api/projects`.
    pub projects: Vec<Value>,
    /// `GET /api/groups`.
    pub groups: Vec<Value>,
    /// The hub URL (for links and the preview ports).
    pub base_url: String,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_screen_id() -> ScreenId {
    NEXT_ID.fetch_add(1, Ordering::SeqCst)
}

impl Ctx {
    /// Run an API call in the background; the result arrives in `Screen::on_task(key, …)`.
    pub fn task<F>(&self, key: impl Into<String>, fut: F)
    where
        F: Future<Output = anyhow::Result<Value>> + Send + 'static,
    {
        let (tx, screen, key) = (self.tx.clone(), self.screen, format!("{}{}", self.prefix, key.into()));
        tokio::spawn(async move {
            let result = fut.await;
            let _ = tx.send(AppEvent::Task { screen, key, result });
        });
    }

    /// GET a path as JSON in the background.
    pub fn get(&self, key: impl Into<String>, path: impl Into<String>) {
        let (c, path) = (self.client.clone(), path.into());
        self.task(key, async move { c.get::<Value>(&path).await });
    }

    /// A mutating request in the background (JSON body; `Value::Null` sends `{}`).
    pub fn send(&self, key: impl Into<String>, method: hubness_client::Method, path: impl Into<String>, body: Value) {
        let (c, path) = (self.client.clone(), path.into());
        let body = if body.is_null() { serde_json::json!({}) } else { body };
        self.task(key, async move { c.send::<Value, _>(method, &path, &body).await });
    }

    /// Open (or replace) a live SSE stream; messages arrive in `Screen::on_stream(key, …)`.
    pub fn stream(&mut self, key: impl Into<String>, path: impl Into<String>) {
        let key = format!("{}{}", self.prefix, key.into());
        if let Some(h) = self.streams.remove(&(self.screen, key.clone())) {
            h.abort();
        }
        let mut rx = self.client.stream::<Value>(&path.into());
        let (tx, screen, k) = (self.tx.clone(), self.screen, key.clone());
        let handle = tokio::spawn(async move {
            let mut error = None;
            while let Some(m) = rx.recv().await {
                match m {
                    Ok(msg) => {
                        if tx.send(AppEvent::Stream { screen, key: k.clone(), msg }).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        error = Some(e.to_string());
                        break;
                    }
                }
            }
            let _ = tx.send(AppEvent::StreamEnded { screen, key: k, error });
        });
        self.streams.insert((self.screen, key), handle);
    }

    pub fn close_stream(&mut self, key: &str) {
        if let Some(h) = self.streams.remove(&(self.screen, format!("{}{key}", self.prefix))) {
            h.abort();
        }
    }

    pub fn go(&mut self, route: Route) {
        self.requests.push(Request::Go(route));
    }

    pub fn back(&mut self) {
        self.requests.push(Request::Back);
    }

    pub fn modal(&mut self, m: Modal) {
        let p = self.prefix.clone();
        let m = match m {
            Modal::Input { title, value, key, secret, multiline } => {
                Modal::Input { title, value, key: format!("{p}{key}"), secret, multiline }
            }
            Modal::Confirm { title, message, key, danger } => Modal::Confirm { title, message, key: format!("{p}{key}"), danger },
            Modal::Pick { title, items, filter, selected, key } => Modal::Pick { title, items, filter, selected, key: format!("{p}{key}") },
            other => other,
        };
        self.requests.push(Request::Modal(m));
    }

    /// Ask for multi-line text (Enter saves, Alt+Enter newline); the answer comes to
    /// `on_task(key, Ok(String | Null))`.
    pub fn prompt_multiline(&mut self, key: &str, title: impl Into<String>, value: impl Into<String>) {
        self.modal(Modal::Input { title: title.into(), value: value.into(), key: key.into(), secret: false, multiline: true });
    }

    /// Ask for a secret (shown as dots).
    pub fn prompt_secret(&mut self, key: &str, title: impl Into<String>) {
        self.modal(Modal::Input { title: title.into(), value: String::new(), key: key.into(), secret: true, multiline: false });
    }

    /// Show a read-only message (closed with Enter / Esc).
    pub fn message(&mut self, title: impl Into<String>, text: impl Into<String>) {
        self.modal(Modal::Message { title: title.into(), text: text.into() });
    }

    /// Ask for a line of text; the answer comes to `on_task(key, Ok(String | Null))`.
    pub fn prompt(&mut self, key: &str, title: impl Into<String>, value: impl Into<String>) {
        self.modal(Modal::Input { title: title.into(), value: value.into(), key: key.into(), secret: false, multiline: false });
    }

    /// Ask yes/no; the answer comes to `on_task(key, Ok(Bool))`.
    pub fn confirm(&mut self, key: &str, title: impl Into<String>, message: impl Into<String>, danger: bool) {
        self.modal(Modal::Confirm { title: title.into(), message: message.into(), key: key.into(), danger });
    }

    /// Pick one of `items` (id, label); the id comes to `on_task(key, Ok(String | Null))`.
    pub fn pick(&mut self, key: &str, title: impl Into<String>, items: Vec<(String, String)>) {
        self.modal(Modal::Pick { title: title.into(), items, filter: String::new(), selected: 0, key: key.into() });
    }

    /// Make `area` clickable while drawing (cleared every frame).
    pub fn click(&self, area: Rect, hit: Hit) {
        let hit = match hit {
            Hit::Click(id) => Hit::Click(format!("{}{id}", self.draw_prefix.borrow())),
            other => other,
        };
        self.hits.borrow_mut().push((area, self.screen, hit));
    }

    /// Rows of a list drawn in `inner` (one line each, scrolled by `offset`): a click on row `i`
    /// reaches `on_click("<id>:<index>")`.
    pub fn click_rows(&self, id: &str, inner: Rect, offset: usize, count: usize) {
        for row in 0..(inner.height as usize).min(count.saturating_sub(offset)) {
            let r = Rect { x: inner.x, y: inner.y + row as u16, width: inner.width, height: 1 };
            self.click(r, Hit::Click(format!("{id}:{}", offset + row)));
        }
    }

    /// Leave this hub for a saved one (`None` — the connect screen, to add one).
    pub fn switch_hub(&mut self, url: Option<String>) {
        self.requests.push(Request::SwitchHub(url));
    }

    /// Log out of the hub (the app shows the sign-in screen).
    pub fn log_out(&mut self) {
        self.requests.push(Request::LogOut);
    }

    pub fn open_url(&mut self, url: impl Into<String>) {
        self.requests.push(Request::OpenUrl(url.into()));
    }

    pub fn info(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), false));
    }

    pub fn error(&mut self, msg: impl std::fmt::Display) {
        self.status = Some((msg.to_string(), true));
    }

    /// Report a failed task in the status line; `Some(value)` when it succeeded.
    pub fn ok(&mut self, result: anyhow::Result<Value>) -> Option<Value> {
        match result {
            Ok(v) => Some(v),
            Err(e) => {
                self.error(e);
                None
            }
        }
    }
}

/// A context for unit tests: no hub behind it (requests fail in the background) and nobody
/// reading the events.
#[cfg(test)]
pub(crate) fn test_ctx() -> Ctx {
    let (tx, _rx) = mpsc::unbounded_channel();
    Ctx {
        client: Client::new("http://127.0.0.1:1", None).expect("a client"),
        tx,
        screen: 1,
        requests: vec![],
        streams: HashMap::new(),
        status: None,
        prefix: String::new(),
        hits: Default::default(),
        draw_prefix: Default::default(),
        shared: Shared::default(),
    }
}
