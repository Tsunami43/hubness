//! The app shell: sidebar (sections and sessions), the active screen, the footer, the
//! leader (which-key) panel and modals. Mirrors the web app's layout and leader keys.

use std::collections::HashMap;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, Paragraph, Wrap};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::core::{Action, AppEvent, Ctx, Handled, Modal, Request, Route, Screen, ScreenId, Section, Shared, next_screen_id};
use crate::screens;
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{self, Theme, block, centered};

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
    leader: bool,
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
            leader: false,
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

    fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if self.modal.is_some() {
            return self.on_modal_key(k);
        }
        if self.leader {
            self.leader = false;
            return self.on_leader_key(k);
        }
        // the leader: Ctrl+Space or Ctrl+G (some terminals can't send Ctrl+Space)
        if ctrl && matches!(k.code, KeyCode::Char(' ') | KeyCode::Char('g')) {
            self.leader = true;
            return;
        }
        if ctrl && k.code == KeyCode::Char('q') {
            self.quit = true;
            return;
        }
        let handled = self.with_screen(|s, ctx| s.on_key(k, ctx)).unwrap_or(Handled::No);
        if matches!(handled, Handled::Yes) {
            return;
        }
        let typing = self.screens.get(&self.route).is_some_and(|s| s.screen.captures_text());
        match k.code {
            KeyCode::Char('c') if ctrl => self.quit = true,
            KeyCode::Char('b') if ctrl => self.sidebar = !self.sidebar,
            KeyCode::Left if k.modifiers.contains(KeyModifiers::ALT) => self.ctx.back(),
            KeyCode::Esc if !self.history.is_empty() => self.ctx.back(),
            KeyCode::Char('q') if !typing => self.quit = true,
            KeyCode::Char(c) if !typing && !ctrl => {
                // a screen action by its key
                let key = c.to_string();
                if let Some(a) = self.screen_actions().into_iter().find(|a| a.key == key) {
                    self.with_screen(|s, ctx| s.on_action(a.key, ctx));
                }
            }
            _ => {}
        }
        self.apply_requests();
    }

    fn screen_actions(&self) -> Vec<Action> {
        self.screens.get(&self.route).map(|s| s.screen.actions()).unwrap_or_default()
    }

    /// Global leader actions (as the web app's leader panel).
    fn leader_actions(&self) -> Vec<(&'static str, &'static str, Option<Route>)> {
        vec![
            ("c", "Chats", Some(Route::Home)),
            ("p", "Projects", Some(Route::Projects)),
            ("s", "System", Some(Route::System(None))),
            ("h", "Harness (agents)", Some(Route::Harness(None))),
            (",", "Settings", Some(Route::Settings(None))),
            ("j", "Jump to a session…", None),
            ("b", "Toggle sidebar", None),
            ("r", "Reload", None),
            ("q", "Quit", None),
        ]
    }

    fn on_leader_key(&mut self, k: KeyEvent) {
        let KeyCode::Char(c) = k.code else { return };
        let key = c.to_string();
        if let Some((_, _, route)) = self.leader_actions().into_iter().find(|(kk, _, _)| *kk == key) {
            match (key.as_str(), route) {
                (_, Some(r)) => self.go(r),
                ("j", _) => {
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
                ("b", _) => self.sidebar = !self.sidebar,
                ("r", _) => {
                    self.refresh_shared();
                    self.with_screen(|s, ctx| s.on_show(ctx));
                }
                ("q", _) => self.quit = true,
                _ => {}
            }
            return;
        }
        if let Some(a) = self.screen_actions().into_iter().find(|a| a.key == key) {
            self.with_screen(|s, ctx| s.on_action(a.key, ctx));
        }
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
                if key == "jump"
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

    pub fn draw(&mut self, f: &mut Frame) {
        let t = Theme::get();
        let [main, footer] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(f.area());
        let content = if self.sidebar && main.width > 70 {
            let [side, content] = Layout::horizontal([Constraint::Length(30), Constraint::Min(30)]).areas(main);
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
        // footer: status (or hints) on the left, the leader key on the right
        let status = match &self.ctx.status {
            Some((msg, true)) => Line::styled(format!(" {msg}"), Style::default().fg(t.danger)),
            Some((msg, false)) => Line::styled(format!(" {msg}"), t.muted_style()),
            None => {
                let mut pairs: Vec<(String, String)> =
                    self.screen_actions().into_iter().take(6).map(|a| (a.key.to_string(), a.label)).collect();
                pairs.insert(0, ("^␣".into(), "leader".into()));
                let refs: Vec<(&str, &str)> = pairs.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
                ui::hints(&refs)
            }
        };
        f.render_widget(Paragraph::new(status), footer);
        if self.leader {
            self.draw_leader(f, main);
        }
        if let Some((_, modal, input)) = &self.modal {
            draw_modal(f, main, modal, input);
        }
    }

    fn draw_sidebar(&self, f: &mut Frame, area: Rect) {
        let t = Theme::get();
        let section = self.route.section();
        let mut items: Vec<ListItem> = vec![ListItem::new(Line::styled(" hubness", t.accent_style().add_modifier(Modifier::BOLD)))];
        for (s, label, key) in [
            (Section::Chats, "Chats", "c"),
            (Section::Projects, "Projects", "p"),
            (Section::System, "System", "s"),
            (Section::Harness, "Harness", "h"),
            (Section::Settings, "Settings", ","),
        ] {
            let style = if s == section { t.accent_style().add_modifier(Modifier::BOLD) } else { Style::default() };
            items.push(ListItem::new(Line::from(vec![
                Span::styled(format!(" {} ", if s == section { '▸' } else { ' ' }), t.accent_style()),
                Span::styled(label, style),
                Span::styled(format!("  {key}"), t.muted_style()),
            ])));
        }
        items.push(ListItem::new(Line::raw("")));
        items.push(ListItem::new(Line::styled(" Sessions", t.muted_style())));
        let current = match &self.route {
            Route::Chat(id) | Route::ProjectSession { session: id, .. } | Route::GroupSession { session: id, .. } => Some(id.as_str()),
            _ => None,
        };
        let room = area.height.saturating_sub(10) as usize;
        for s in self.ctx.shared.sessions.iter().take(room) {
            let (mark, color) = t.status(s["status"].as_str().unwrap_or("idle"));
            let selected = current == s["id"].as_str();
            let title = s["title"].as_str().unwrap_or_default();
            let style = if selected { t.selected() } else { Style::default() };
            items.push(ListItem::new(Line::from(vec![
                Span::styled(format!(" {mark} "), Style::default().fg(color)),
                Span::styled(title.to_string(), style),
            ])));
        }
        f.render_widget(List::new(items).block(block("", false)), area);
    }

    fn draw_leader(&self, f: &mut Frame, area: Rect) {
        let t = Theme::get();
        let screen = self.screen_actions();
        let mut left: Vec<Line> = vec![Line::styled("GO", t.muted_style())];
        for (k, label, _) in self.leader_actions() {
            left.push(Line::from(vec![Span::styled(format!(" {k} "), t.key_style()), Span::raw(format!(" {label}"))]));
        }
        let mut right: Vec<Line> =
            vec![Line::styled(self.screens.get(&self.route).map(|s| s.screen.title().to_uppercase()).unwrap_or_default(), t.muted_style())];
        for a in &screen {
            right.push(Line::from(vec![Span::styled(format!(" {} ", a.key), t.key_style()), Span::raw(format!(" {}", a.label))]));
        }
        let h = (left.len().max(right.len()) as u16 + 3).min(area.height);
        let rect =
            Rect { x: area.x + 1, y: area.y + area.height.saturating_sub(h), width: area.width.saturating_sub(2).min(90), height: h };
        f.render_widget(Clear, rect);
        let b = block("leader — press a key, Esc to cancel", true);
        let inner = b.inner(rect);
        f.render_widget(b, rect);
        let [l, r] = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(inner);
        f.render_widget(Paragraph::new(left), l);
        f.render_widget(Paragraph::new(right), r);
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
            let lines = message.lines().count() as u16 + 4;
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
