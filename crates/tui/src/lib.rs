//! Terminal UI for hubness: a client of the hub's HTTP/SSE API (the same one the web app
//! uses), so chats started here show up in the browser and vice versa.
//!
//! Layout: sessions on the left (live statuses), the selected session's feed on the right
//! (streamed as the agent works), a message box at the bottom.

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use hubness_client::Client;
use hubness_core::{GlobalStreamMsg, Session, SessionStatus, SessionStreamMsg};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use serde_json::Value;
use tokio::sync::mpsc;

enum AppEvent {
    Input(Event),
    Global(GlobalStreamMsg),
    Session(String, SessionStreamMsg),
    Done(Result<String, String>),
    /// A chat created from the TUI: select and open it.
    Created(Session),
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Focus {
    List,
    Input,
}

struct App {
    client: Client,
    tx: mpsc::UnboundedSender<AppEvent>,
    sessions: Vec<Session>,
    list: ListState,
    focus: Focus,
    /// The open session and its feed.
    current: Option<Session>,
    feed: Vec<Value>,
    /// Lines scrolled up from the bottom of the feed.
    scroll: u16,
    input: String,
    /// The next message starts a new chat instead of going to the open session.
    composing_new: bool,
    default_agent: String,
    status: String,
    stream_task: Option<tokio::task::JoinHandle<()>>,
    quit: bool,
}

/// Run the TUI against the hub at `url` until the user quits.
pub async fn run(url: &str) -> anyhow::Result<()> {
    let client = Client::new(url)?;
    client.health().await.map_err(|e| anyhow::anyhow!("hubness is not reachable at {url}: {e}"))?;
    let sessions = client.sessions().await?;
    let default_agent = client.default_agent().await.unwrap_or_else(|_| "claude-code".into());
    let (tx, mut rx) = mpsc::unbounded_channel();

    // global session changes keep the list live
    let mut global = client.global_stream();
    let gtx = tx.clone();
    tokio::spawn(async move {
        while let Some(Ok(m)) = global.recv().await {
            if gtx.send(AppEvent::Global(m)).is_err() {
                break;
            }
        }
    });
    let itx = tx.clone();
    tokio::spawn(async move {
        let mut events = EventStream::new();
        while let Some(Ok(e)) = events.next().await {
            if itx.send(AppEvent::Input(e)).is_err() {
                break;
            }
        }
    });

    let mut app = App {
        client,
        tx,
        sessions,
        list: ListState::default(),
        focus: Focus::List,
        current: None,
        feed: vec![],
        scroll: 0,
        input: String::new(),
        composing_new: false,
        default_agent,
        status: "n new chat · Enter open · Tab focus · ^Y/^N approve/deny · ^S stop · q quit".into(),
        stream_task: None,
        quit: false,
    };
    if !app.sessions.is_empty() {
        app.list.select(Some(0));
    }

    let mut terminal = ratatui::init();
    let result = async {
        loop {
            terminal.draw(|f| app.draw(f))?;
            let Some(ev) = rx.recv().await else { break };
            app.handle(ev);
            // apply everything already queued before redrawing (streaming deltas come in bursts)
            while let Ok(ev) = rx.try_recv() {
                app.handle(ev);
            }
            if app.quit {
                break;
            }
        }
        anyhow::Ok(())
    }
    .await;
    ratatui::restore();
    result
}

impl App {
    fn handle(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Input(Event::Key(k)) if k.kind == KeyEventKind::Press => self.on_key(k),
            AppEvent::Input(_) => {}
            AppEvent::Global(GlobalStreamMsg::Session { session }) => self.upsert_session(session),
            AppEvent::Global(GlobalStreamMsg::SessionDeleted { id }) => {
                self.sessions.retain(|s| s.id != id);
                self.clamp_selection();
            }
            AppEvent::Session(id, msg) if self.current.as_ref().is_some_and(|c| c.id == id) => self.on_session_msg(msg),
            AppEvent::Session(..) => {}
            AppEvent::Done(Ok(msg)) => self.status = msg,
            AppEvent::Done(Err(e)) => self.status = format!("error: {e}"),
            AppEvent::Created(s) => {
                self.upsert_session(s.clone());
                if let Some(i) = self.sessions.iter().position(|x| x.id == s.id) {
                    self.list.select(Some(i));
                }
                self.status = format!("new chat with {}", s.agent.kind);
                self.open(s);
            }
        }
    }

    fn upsert_session(&mut self, s: Session) {
        if s.archived_at.is_some() {
            self.sessions.retain(|x| x.id != s.id);
        } else if let Some(x) = self.sessions.iter_mut().find(|x| x.id == s.id) {
            *x = s.clone();
        } else {
            self.sessions.insert(0, s.clone());
            if let Some(i) = self.list.selected() {
                self.list.select(Some(i + 1));
            }
        }
        if self.current.as_ref().is_some_and(|c| c.id == s.id) {
            self.current = Some(s);
        }
        self.clamp_selection();
    }

    fn clamp_selection(&mut self) {
        match (self.sessions.len(), self.list.selected()) {
            (0, _) => self.list.select(None),
            (n, Some(i)) if i >= n => self.list.select(Some(n - 1)),
            (_, None) => self.list.select(Some(0)),
            _ => {}
        }
    }

    fn on_session_msg(&mut self, msg: SessionStreamMsg) {
        match msg {
            SessionStreamMsg::Snapshot { session, events, .. } => {
                self.current = Some(session);
                self.feed = events;
            }
            SessionStreamMsg::Event { event } => {
                match self.feed.iter_mut().find(|e| e["uid"] == event["uid"]) {
                    Some(e) => *e = event,
                    None => self.feed.push(event),
                }
                self.feed.sort_by_key(|e| e["seq"].as_i64().unwrap_or(0));
            }
            SessionStreamMsg::Session { session } => self.current = Some(session),
            SessionStreamMsg::Pane { .. } => {}
        }
    }

    /// Open a session: subscribe to its live stream (the snapshot fills the feed).
    fn open(&mut self, s: Session) {
        if let Some(t) = self.stream_task.take() {
            t.abort();
        }
        self.feed.clear();
        self.scroll = 0;
        self.composing_new = false;
        let mut rx = self.client.session_stream(&s.id);
        let tx = self.tx.clone();
        let id = s.id.clone();
        self.stream_task = Some(tokio::spawn(async move {
            while let Some(m) = rx.recv().await {
                match m {
                    Ok(m) => {
                        if tx.send(AppEvent::Session(id.clone(), m)).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::Done(Err(e.to_string())));
                        break;
                    }
                }
            }
        }));
        self.current = Some(s);
    }

    /// Run an API call in the background; its outcome goes to the status line.
    fn spawn<F>(&self, fut: F)
    where
        F: std::future::Future<Output = anyhow::Result<String>> + Send + 'static,
    {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(AppEvent::Done(fut.await.map_err(|e| e.to_string())));
        });
    }

    fn pending_approval(&self) -> Option<String> {
        self.feed
            .iter()
            .rev()
            .find(|e| e["kind"] == "approval" && e["status"] == "pending")
            .and_then(|e| e["requestId"].as_str().map(String::from))
    }

    fn approve(&mut self, allow: bool) {
        let (Some(s), Some(rid)) = (self.current.clone(), self.pending_approval()) else {
            self.status = "nothing to approve".into();
            return;
        };
        let c = self.client.clone();
        self.spawn(async move {
            c.approve(&s.id, &rid, allow).await?;
            Ok(if allow { "allowed" } else { "denied" }.into())
        });
    }

    fn submit(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.input.clear();
        let c = self.client.clone();
        match self.current.clone().filter(|_| !self.composing_new) {
            Some(s) => self.spawn(async move {
                c.send_message(&s.id, &text).await?;
                Ok("sent".into())
            }),
            None => {
                // a new chat: create it with the prompt, then open it
                let agent = self.default_agent.clone();
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    match c.create_session(None, &agent, None, Some(&text)).await {
                        Ok(s) => {
                            let _ = tx.send(AppEvent::Created(s));
                        }
                        Err(e) => {
                            let _ = tx.send(AppEvent::Done(Err(e.to_string())));
                        }
                    }
                });
                self.status = "creating a chat…".into();
            }
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match (k.code, ctrl) {
            (KeyCode::Char('c'), true) => self.quit = true,
            (KeyCode::Char('y'), true) => self.approve(true),
            (KeyCode::Char('n'), true) => self.approve(false),
            (KeyCode::Char('s'), true) => {
                if let Some(s) = self.current.clone() {
                    let c = self.client.clone();
                    self.spawn(async move { Ok(if c.stop(&s.id).await.is_ok() { "stopping" } else { "nothing to stop" }.into()) });
                }
            }
            (KeyCode::Tab, _) => self.focus = if self.focus == Focus::List { Focus::Input } else { Focus::List },
            (KeyCode::PageUp, _) => self.scroll = self.scroll.saturating_add(10),
            (KeyCode::PageDown, _) => self.scroll = self.scroll.saturating_sub(10),
            _ if self.focus == Focus::Input => self.on_input_key(k),
            _ => self.on_list_key(k),
        }
    }

    fn on_list_key(&mut self, k: KeyEvent) {
        let n = self.sessions.len();
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Down | KeyCode::Char('j') if n > 0 => self.list.select(Some((self.list.selected().unwrap_or(0) + 1).min(n - 1))),
            KeyCode::Up | KeyCode::Char('k') if n > 0 => self.list.select(Some(self.list.selected().unwrap_or(0).saturating_sub(1))),
            KeyCode::Enter => {
                if let Some(s) = self.list.selected().and_then(|i| self.sessions.get(i)).cloned() {
                    self.open(s);
                    self.focus = Focus::Input;
                }
            }
            KeyCode::Char('n') => {
                self.composing_new = true;
                self.focus = Focus::Input;
                self.status = format!("new chat with {} — type the first message", self.default_agent);
            }
            KeyCode::Char('y') => self.approve(true),
            KeyCode::Char('a') => {
                if let Some(s) = self.list.selected().and_then(|i| self.sessions.get(i)).cloned() {
                    let c = self.client.clone();
                    self.spawn(async move {
                        c.archive(&s.id).await?;
                        Ok(format!("archived “{}”", s.title))
                    });
                }
            }
            _ => {}
        }
    }

    fn on_input_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Esc => {
                self.focus = Focus::List;
                self.composing_new = false;
            }
            KeyCode::Enter if k.modifiers.contains(KeyModifiers::ALT) => self.input.push('\n'),
            KeyCode::Enter => self.submit(),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(c) => self.input.push(c),
            _ => {}
        }
    }

    // ---------- drawing ----------

    fn draw(&mut self, f: &mut Frame) {
        let rows =
            Layout::default().direction(Direction::Vertical).constraints([Constraint::Min(3), Constraint::Length(1)]).split(f.area());
        let cols =
            Layout::default().direction(Direction::Horizontal).constraints([Constraint::Length(36), Constraint::Min(20)]).split(rows[0]);
        self.draw_list(f, cols[0]);
        let right =
            Layout::default().direction(Direction::Vertical).constraints([Constraint::Min(3), Constraint::Length(5)]).split(cols[1]);
        self.draw_feed(f, right[0]);
        self.draw_input(f, right[1]);
        f.render_widget(Paragraph::new(self.status.clone()).style(Style::default().fg(Color::DarkGray)), rows[1]);
    }

    fn border(&self, focus: Focus, title: String) -> Block<'static> {
        let style = if self.focus == focus { Style::default().fg(Color::Cyan) } else { Style::default().fg(Color::DarkGray) };
        Block::default().borders(Borders::ALL).border_style(style).title(title)
    }

    fn draw_list(&mut self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .sessions
            .iter()
            .map(|s| {
                let (mark, color) = match s.status {
                    SessionStatus::Running => ("●", Color::Green),
                    SessionStatus::Waiting => ("?", Color::Yellow),
                    SessionStatus::Error => ("!", Color::Red),
                    SessionStatus::Idle => ("·", Color::DarkGray),
                };
                let scope = if s.project_id.is_some() {
                    "proj"
                } else if s.group_id.is_some() {
                    "group"
                } else {
                    "chat"
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{mark} "), Style::default().fg(color)),
                    Span::raw(s.title.clone()),
                    Span::styled(format!("  {scope} · {}", s.agent.kind), Style::default().fg(Color::DarkGray)),
                ]))
            })
            .collect();
        let list = List::new(items)
            .block(self.border(Focus::List, format!(" Sessions ({}) ", self.sessions.len())))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        f.render_stateful_widget(list, area, &mut self.list);
    }

    fn draw_feed(&self, f: &mut Frame, area: Rect) {
        let title = match &self.current {
            Some(s) if !self.composing_new => format!(" {} · {} · {} ", s.title, s.agent.kind, format!("{:?}", s.status).to_lowercase()),
            _ => " new chat ".into(),
        };
        let block = self.border(Focus::Input, title).border_style(Style::default().fg(Color::DarkGray));
        let width = area.width.saturating_sub(2).max(10) as usize;
        let mut lines: Vec<Line> = vec![];
        if !self.composing_new {
            for e in &self.feed {
                for l in event_lines(e) {
                    lines.extend(wrap(l, width));
                }
            }
        }
        let height = area.height.saturating_sub(2) as usize;
        let max_scroll = lines.len().saturating_sub(height);
        let offset = max_scroll.saturating_sub(self.scroll as usize);
        let visible: Vec<Line> = lines.into_iter().skip(offset).take(height).collect();
        f.render_widget(Paragraph::new(visible).block(block), area);
    }

    fn draw_input(&self, f: &mut Frame, area: Rect) {
        let hint = if self.composing_new || self.current.is_none() {
            " message (Enter — new chat) "
        } else {
            " message (Enter send · Alt+Enter newline · Esc back) "
        };
        let text = if self.focus == Focus::Input { format!("{}▏", self.input) } else { self.input.clone() };
        f.render_widget(
            Paragraph::new(text).block(self.border(Focus::Input, hint.into())).wrap(ratatui::widgets::Wrap { trim: false }),
            area,
        );
    }
}

/// A short summary of a tool's input: the command, path, pattern or URL.
fn tool_summary(input: &Value) -> String {
    for k in ["command", "file_path", "pattern", "url", "path", "description", "query"] {
        if let Some(s) = input[k].as_str() {
            return s.lines().next().unwrap_or_default().to_string();
        }
    }
    String::new()
}

/// Feed event → styled lines (before wrapping).
fn event_lines(e: &Value) -> Vec<Line<'static>> {
    let text = |k: &str| e[k].as_str().unwrap_or_default().to_string();
    match e["kind"].as_str().unwrap_or_default() {
        "user" => {
            let mut out = vec![Line::raw("")];
            for (i, l) in text("text").lines().enumerate() {
                let prefix = if i == 0 { "› " } else { "  " };
                out.push(Line::from(Span::styled(format!("{prefix}{l}"), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))));
            }
            if let Some(files) = e["attachments"].as_array() {
                for a in files {
                    out.push(Line::from(Span::styled(
                        format!("  📎 {}", a["name"].as_str().unwrap_or_default()),
                        Style::default().fg(Color::Cyan),
                    )));
                }
            }
            out
        }
        "text" => {
            let mut out: Vec<Line> = text("text").lines().map(|l| Line::raw(l.to_string())).collect();
            if e["streaming"] == true {
                out.push(Line::from(Span::styled("▍", Style::default().fg(Color::Green))));
            }
            out
        }
        "thinking" => text("text")
            .lines()
            .take(3)
            .map(|l| Line::from(Span::styled(format!("  ∴ {l}"), Style::default().fg(Color::DarkGray).italic())))
            .collect(),
        "tool" => {
            let (mark, color) = match e["status"].as_str() {
                Some("ok") => ("✓", Color::Green),
                Some("error") => ("✗", Color::Red),
                Some("denied") => ("⊘", Color::Yellow),
                _ => ("…", Color::Blue),
            };
            vec![Line::from(vec![
                Span::styled(format!("  {mark} "), Style::default().fg(color)),
                Span::styled(text("name"), Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(format!(" {}", tool_summary(&e["input"])), Style::default().fg(Color::DarkGray)),
            ])]
        }
        "approval" => {
            let pending = e["status"] == "pending";
            let tail = if pending { "  allow? Ctrl+Y / deny Ctrl+N".to_string() } else { format!("  {}", text("status")) };
            vec![Line::from(vec![
                Span::styled("  ? ", Style::default().fg(Color::Yellow)),
                Span::styled(format!("{} {}", text("toolName"), tool_summary(&e["input"])), Style::default().fg(Color::Yellow)),
                Span::styled(
                    tail,
                    Style::default().fg(if pending { Color::Yellow } else { Color::DarkGray }).add_modifier(if pending {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
                ),
            ])]
        }
        "notice" => {
            let color = if e["level"] == "error" { Color::Red } else { Color::DarkGray };
            text("text").lines().map(|l| Line::from(Span::styled(format!("  {l}"), Style::default().fg(color)))).collect()
        }
        "result" => {
            let mut bits = vec![if e["ok"] == true {
                "done".to_string()
            } else if e["stopped"] == true {
                "stopped".into()
            } else {
                "failed".into()
            }];
            if let Some(ms) = e["durationMs"].as_f64() {
                bits.push(format!("{:.1}s", ms / 1000.0));
            }
            if e["billing"] == "api"
                && let Some(c) = e["costUsd"].as_f64()
            {
                bits.push(format!("${c:.2}"));
            }
            if let Some(m) = e["model"].as_str() {
                bits.push(m.to_string());
            }
            vec![Line::from(Span::styled(format!("  — {}", bits.join(" · ")), Style::default().fg(Color::DarkGray)))]
        }
        _ => vec![],
    }
}

/// Hard-wrap a line to `width` columns (counting chars), keeping span styles.
fn wrap(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    let total: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
    if total <= width {
        return vec![line];
    }
    let mut out = vec![];
    let mut cur: Vec<Span<'static>> = vec![];
    let mut used = 0;
    for span in line.spans {
        let mut rest: Vec<char> = span.content.chars().collect();
        while !rest.is_empty() {
            let room = width - used;
            let take = room.min(rest.len());
            let piece: String = rest.drain(..take).collect();
            cur.push(Span::styled(piece, span.style));
            used += take;
            if used == width {
                out.push(Line::from(std::mem::take(&mut cur)));
                used = 0;
            }
        }
    }
    if !cur.is_empty() {
        out.push(Line::from(cur));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wraps_long_lines() {
        let lines = wrap(Line::raw("abcdefghij"), 4);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[2].spans[0].content, "ij");
    }

    #[test]
    fn renders_feed_events() {
        let tool = json!({ "kind": "tool", "name": "Bash", "input": { "command": "ls -la\nmore" }, "status": "ok" });
        let l = &event_lines(&tool)[0];
        assert!(l.spans.iter().any(|s| s.content.contains("ls -la")));
        let ap = json!({ "kind": "approval", "toolName": "Bash", "input": {}, "status": "pending", "requestId": "r" });
        assert!(event_lines(&ap)[0].spans.iter().any(|s| s.content.contains("Ctrl+Y")));
        assert!(
            event_lines(&json!({ "kind": "result", "ok": true, "durationMs": 1500, "model": "m" }))[0].spans[0].content.contains("1.5s")
        );
    }
}
