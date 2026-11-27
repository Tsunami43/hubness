//! A session (the web app's SessionView): the live feed, the message box with attachments,
//! approvals (incl. AskUserQuestion), model / permission mode, the pane next to the chat,
//! stop, rename, archive and moving an Inbox chat into a project.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use serde_json::{Value, json};

use super::feed::{Detail, event_lines};
use crate::core::{Action, Ctx, Handled, Route, Screen};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Theme, block, code, markdown, tail_range, wrap};

pub const MODES: &[(&str, &str)] = &[
    ("ask", "Ask — edits and commands need approval"),
    ("acceptEdits", "Accept edits — commands still ask"),
    ("bypass", "Bypass — never ask"),
    ("plan", "Plan — read only"),
];

#[derive(PartialEq, Eq, Clone, Copy)]
enum Focus {
    Composer,
    Feed,
}

pub struct SessionView {
    id: String,
    session: Value,
    events: Vec<Value>,
    pane: Value,
    /// The pane's content (file or diff), loaded on demand.
    pane_body: Option<Vec<Line<'static>>>,
    show_pane: bool,
    composer: TextInput,
    attachments: Vec<Value>,
    focus: Focus,
    scroll: usize,
    detail: Detail,
    suggestion: Value,
    /// AskUserQuestion answers being collected: (request id, questions, answers so far).
    asking: Option<(String, Vec<Value>, serde_json::Map<String, Value>)>,
    loaded: bool,
    /// Rendered (markdown, highlighted) and wrapped lines per event uid, so a frame only
    /// renders events that changed. Valid for `feed_key`.
    feed_cache: HashMap<String, Vec<Line<'static>>>,
    feed_key: Option<(Detail, u16, Theme)>,
}

impl SessionView {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            session: Value::Null,
            events: vec![],
            pane: Value::Null,
            pane_body: None,
            show_pane: false,
            composer: TextInput::multiline(),
            attachments: vec![],
            // NORMAL first, like Vim: i (or a, o, Enter) starts typing
            focus: Focus::Feed,
            scroll: 0,
            detail: Detail::Compact,
            suggestion: Value::Null,
            asking: None,
            loaded: false,
            feed_cache: HashMap::new(),
            feed_key: None,
        }
    }

    fn path(&self, rest: &str) -> String {
        format!("/sessions/{}{rest}", self.id)
    }

    fn pending(&self) -> Option<&Value> {
        self.events.iter().rev().find(|e| e["kind"] == "approval" && e["status"] == "pending")
    }

    fn upsert(&mut self, ev: Value) {
        self.feed_cache.remove(&event_key(&ev, self.events.len()));
        match self.events.iter_mut().find(|e| e["uid"] == ev["uid"]) {
            Some(e) => *e = ev,
            None => self.events.push(ev),
        }
        self.events.sort_by_key(|e| e["seq"].as_i64().unwrap_or(0));
    }

    fn send(&mut self, ctx: &mut Ctx) {
        let text = self.composer.value();
        if text.trim().is_empty() && self.attachments.is_empty() {
            return;
        }
        let files: Vec<Value> = self.attachments.iter().map(|a| json!({ "id": a["id"], "name": a["name"], "mime": a["mime"] })).collect();
        ctx.send("sent", Method::POST, self.path("/messages"), json!({ "text": text, "attachments": files }));
        self.composer.clear();
        self.attachments.clear();
        self.scroll = 0;
    }

    fn decide(&mut self, allow: bool, ctx: &mut Ctx) {
        let Some(p) = self.pending().cloned() else {
            ctx.info("nothing to approve");
            return;
        };
        let rid = p["requestId"].as_str().unwrap_or_default().to_string();
        if allow && let Some(qs) = p["questions"].as_array().filter(|q| !q.is_empty()) {
            // AskUserQuestion: pick an option for each question, then allow with the answers
            self.asking = Some((rid, qs.clone(), serde_json::Map::new()));
            self.ask_next(ctx);
            return;
        }
        let body =
            if allow { json!({ "decision": "allow" }) } else { json!({ "decision": "deny", "reason": "The user denied this action" }) };
        ctx.send("decided", Method::POST, self.path(&format!("/approvals/{rid}")), body);
    }

    fn ask_next(&mut self, ctx: &mut Ctx) {
        let Some((rid, qs, answers)) = &self.asking else { return };
        if let Some(q) = qs.get(answers.len()) {
            let mut items: Vec<(String, String)> = q["options"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|o| {
                    let label = o["label"].as_str().unwrap_or_default().to_string();
                    let desc = o["description"].as_str().map(|d| format!(" — {d}")).unwrap_or_default();
                    (label.clone(), format!("{label}{desc}"))
                })
                .collect();
            items.push(("__other".into(), "Other (type an answer)…".into()));
            ctx.pick("answer", q["question"].as_str().unwrap_or("Question"), items);
        } else {
            let body = json!({ "decision": "allow", "answers": answers });
            let rid = rid.clone();
            self.asking = None;
            ctx.send("decided", Method::POST, self.path(&format!("/approvals/{rid}")), body);
        }
    }

    fn load_pane(&mut self, ctx: &mut Ctx) {
        self.pane_body = None;
        let target = self.pane["target"].as_str().unwrap_or_default().to_string();
        let q = enc(&target);
        match self.pane["kind"].as_str() {
            Some("file") => ctx.get("pane:file", self.path(&format!("/file?path={q}"))),
            Some("diff") => ctx.get("pane:diff", self.path(&format!("/diff?path={q}"))),
            _ => {}
        }
    }

    fn attach(&mut self, path: &str, ctx: &mut Ctx) {
        let path = expand(path.trim());
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => return ctx.error(format!("{path}: {e}")),
        };
        let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
        let mime = mime_of(&name).to_string();
        let (c, id) = (ctx.client.clone(), self.id.clone());
        ctx.info(format!("uploading {name}…"));
        ctx.task("attached", async move { c.upload(&id, &name, &mime, bytes).await });
    }

    fn header(&self) -> Line<'static> {
        let t = Theme::get();
        let s = &self.session;
        let (mark, color) = t.status(s["status"].as_str().unwrap_or("idle"));
        let agent = &s["agent"];
        let model = agent["model"].as_str().unwrap_or("default model");
        let mode = agent["permissionMode"].as_str().unwrap_or("ask");
        Line::from(vec![
            Span::styled(format!(" {mark} "), Style::default().fg(color)),
            Span::styled(s["title"].as_str().unwrap_or("…").to_string(), t.bold()),
            Span::styled(format!("   {} · {model} · {mode}", agent["kind"].as_str().unwrap_or_default()), t.muted_style()),
            Span::styled(if s["archivedAt"].is_null() { String::new() } else { "  · archived".into() }, Style::default().fg(t.warn)),
        ])
    }
}

/// `encodeURIComponent` for query values.
pub fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn expand(p: &str) -> String {
    match p.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", std::env::var("HOME").unwrap_or_default()),
        None => p.to_string(),
    }
}

fn mime_of(name: &str) -> &'static str {
    match name.rsplit('.').next().map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("pdf") => "application/pdf",
        Some("txt" | "log") => "text/plain",
        Some("md") => "text/markdown",
        Some("json") => "application/json",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
}

impl Screen for SessionView {
    fn title(&self) -> String {
        self.session["title"].as_str().unwrap_or("Session").to_string()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        if !self.loaded {
            ctx.stream("live", self.path("/stream"));
            self.loaded = true;
        }
        ctx.get("suggestion", self.path("/project-suggestion"));
    }

    fn captures_text(&self) -> bool {
        self.focus == Focus::Composer
    }

    fn actions(&self) -> Vec<Action> {
        let mut a = vec![];
        if self.pending().is_some() {
            a.push(Action::new("y", "Allow / answer"));
            a.push(Action::new("n", "Deny"));
        }
        if self.session["status"] == "running" || self.session["status"] == "waiting" {
            a.push(Action::new("s", "Stop the agent"));
        }
        a.extend([
            Action::new("i", "Write a message"),
            Action::new("f", "Attach a file"),
            Action::new("m", "Model"),
            Action::new("o", "Permission mode"),
            Action::new("d", if self.detail == Detail::Full { "Compact feed" } else { "Full tool output" }),
            Action::new("r", "Rename"),
        ]);
        if !self.pane.is_null() {
            a.push(Action::new("p", if self.show_pane { "Hide the pane" } else { "Show the pane" }));
            if self.pane["kind"] == "url" {
                a.push(Action::new("O", "Open the pane URL in a browser"));
            }
        }
        if self.session["archivedAt"].is_null() {
            a.push(Action::new("a", "Archive"));
        } else {
            a.push(Action::new("a", "Restore from archive"));
        }
        if self.suggestion["suggest"] == true {
            a.push(Action::new("P", "Move to a project"));
        }
        a.push(Action::new("x", "Delete for good"));
        a
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        let kind = self.session["agent"]["kind"].as_str().unwrap_or("claude-code").to_string();
        match key {
            "y" => self.decide(true, ctx),
            "n" => self.decide(false, ctx),
            "s" => ctx.send("stopped", Method::POST, self.path("/stop"), Value::Null),
            "i" => self.focus = Focus::Composer,
            "f" => ctx.prompt("attach", "Attach a local file (path)", ""),
            "m" => ctx.get("models", format!("/agents/{kind}/models")),
            "o" => ctx.pick("mode", "Permission mode", MODES.iter().map(|(id, l)| (id.to_string(), l.to_string())).collect()),
            "d" => self.detail = if self.detail == Detail::Full { Detail::Compact } else { Detail::Full },
            "r" => ctx.prompt("rename", "Rename the session", self.session["title"].as_str().unwrap_or_default()),
            "p" => {
                self.show_pane = !self.show_pane;
                if self.show_pane {
                    self.load_pane(ctx);
                }
            }
            "O" => {
                if let Some(u) = self.pane["target"].as_str() {
                    ctx.open_url(u.to_string());
                }
            }
            "a" if self.session["archivedAt"].is_null() => ctx.send("archived", Method::POST, self.path("/archive"), Value::Null),
            "a" => ctx.send("restored", Method::POST, self.path("/restore"), Value::Null),
            "P" => ctx.prompt("convert", "Project name", self.suggestion["name"].as_str().unwrap_or_default()),
            "x" => ctx.confirm("delete", "Delete the session", "Delete this session for good? The conversation can’t be restored.", true),
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        if self.focus == Focus::Composer {
            // approvals stay one key away while typing
            if k.modifiers.contains(KeyModifiers::CONTROL) {
                match k.code {
                    KeyCode::Char('y') => {
                        self.decide(true, ctx);
                        return Handled::Yes;
                    }
                    KeyCode::Char('n') => {
                        self.decide(false, ctx);
                        return Handled::Yes;
                    }
                    KeyCode::Char('s') => {
                        ctx.send("stopped", Method::POST, self.path("/stop"), Value::Null);
                        return Handled::Yes;
                    }
                    _ => {}
                }
            }
            return match self.composer.on_key(k) {
                InputEvent::Submit => {
                    self.send(ctx);
                    Handled::Yes
                }
                InputEvent::Cancel => {
                    self.focus = Focus::Feed;
                    Handled::Yes
                }
                InputEvent::Changed => Handled::Yes,
                InputEvent::Ignored => match k.code {
                    KeyCode::PageUp => {
                        self.scroll += 10;
                        Handled::Yes
                    }
                    KeyCode::PageDown => {
                        self.scroll = self.scroll.saturating_sub(10);
                        Handled::Yes
                    }
                    _ => Handled::No,
                },
            };
        }
        match k.code {
            KeyCode::Enter if self.pending().is_some() => self.decide(true, ctx),
            KeyCode::Enter | KeyCode::Tab => self.focus = Focus::Composer,
            KeyCode::Up | KeyCode::Char('k') => self.scroll += 1,
            KeyCode::Down | KeyCode::Char('j') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::PageUp => self.scroll += 10,
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Home => self.scroll = usize::MAX / 2,
            KeyCode::End => self.scroll = 0,
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn on_paste(&mut self, text: &str, _ctx: &mut Ctx) {
        self.focus = Focus::Composer;
        self.composer.paste(text);
    }

    fn on_stream(&mut self, _key: &str, msg: Value, ctx: &mut Ctx) {
        match msg["type"].as_str() {
            Some("snapshot") => {
                self.session = msg["session"].clone();
                self.events = msg["events"].as_array().cloned().unwrap_or_default();
                self.feed_cache.clear();
                self.pane = msg["pane"].clone();
            }
            Some("event") => self.upsert(msg["event"].clone()),
            Some("session") => self.session = msg["session"].clone(),
            Some("pane") => {
                self.pane = msg["pane"].clone();
                self.show_pane = !self.pane.is_null();
                if self.show_pane {
                    self.load_pane(ctx);
                }
            }
            Some("__ended") => {
                // the hub restarted or the network dropped: reconnect (the snapshot heals the feed)
                ctx.stream("live", self.path("/stream"));
            }
            _ => {}
        }
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        let Some(v) = ctx.ok(result) else { return };
        match key {
            "suggestion" => self.suggestion = v,
            "attached" => {
                ctx.info(format!("attached {}", v["name"].as_str().unwrap_or_default()));
                self.attachments.push(v);
                self.focus = Focus::Composer;
            }
            "attach" => {
                if let Some(p) = v.as_str().filter(|p| !p.trim().is_empty()) {
                    self.attach(p, ctx);
                }
            }
            "models" => {
                let mut items: Vec<(String, String)> = vec![(String::new(), "Agent default".into())];
                match v["options"].as_array() {
                    Some(opts) => items.extend(opts.iter().map(|o| {
                        let id = o["id"].as_str().unwrap_or_default().to_string();
                        let desc = o["description"].as_str().map(|d| format!(" — {d}")).unwrap_or_default();
                        (id, format!("{}{desc}", o["label"].as_str().unwrap_or_default()))
                    })),
                    None => items.extend(
                        v["models"].as_array().into_iter().flatten().filter_map(|m| m.as_str()).map(|m| (m.to_string(), m.to_string())),
                    ),
                }
                ctx.pick("model", "Model", items);
            }
            "model" => {
                if let Some(m) = v.as_str() {
                    ctx.send("patched", Method::PATCH, self.path(""), json!({ "agent": { "model": m } }));
                }
            }
            "mode" => {
                if let Some(m) = v.as_str() {
                    ctx.send("patched", Method::PATCH, self.path(""), json!({ "agent": { "permissionMode": m } }));
                }
            }
            "rename" => {
                if let Some(t) = v.as_str().filter(|t| !t.trim().is_empty()) {
                    ctx.send("patched", Method::PATCH, self.path(""), json!({ "title": t }));
                }
            }
            "answer" => match v.as_str() {
                None => self.asking = None,
                Some("__other") => {
                    let q = self
                        .asking
                        .as_ref()
                        .and_then(|(_, qs, a)| qs.get(a.len()))
                        .and_then(|q| q["question"].as_str())
                        .unwrap_or("Answer")
                        .to_string();
                    ctx.prompt("answer_text", q, "");
                }
                Some(label) => {
                    if let Some((_, qs, answers)) = &mut self.asking
                        && let Some(q) = qs.get(answers.len()).and_then(|q| q["question"].as_str())
                    {
                        answers.insert(q.to_string(), json!(label));
                    }
                    self.ask_next(ctx);
                }
            },
            "answer_text" => match v.as_str() {
                None => self.asking = None,
                Some(text) => {
                    if let Some((_, qs, answers)) = &mut self.asking
                        && let Some(q) = qs.get(answers.len()).and_then(|q| q["question"].as_str())
                    {
                        answers.insert(q.to_string(), json!(text));
                    }
                    self.ask_next(ctx);
                }
            },
            "convert" => {
                if let Some(name) = v.as_str() {
                    ctx.send("converted", Method::POST, self.path("/convert"), json!({ "name": name }));
                }
            }
            "converted" => {
                let project = v["project"]["id"].as_str().unwrap_or_default().to_string();
                ctx.info(format!("moved to {}", v["project"]["path"].as_str().unwrap_or_default()));
                ctx.go(Route::ProjectSession { project, session: self.id.clone() });
            }
            "delete" => {
                if v == Value::Bool(true) {
                    ctx.send("deleted", Method::DELETE, self.path(""), Value::Null);
                }
            }
            "deleted" => ctx.go(Route::Home),
            "pane:file" => {
                let path = v["path"].as_str().unwrap_or_default();
                self.pane_body = Some(if v["binary"] == true {
                    vec![Line::raw("(binary file)")]
                } else {
                    code::highlight(v["content"].as_str().unwrap_or_default(), path)
                });
            }
            "pane:diff" => self.pane_body = Some(code::diff(v["diff"].as_str().unwrap_or_default())),
            "archived" => ctx.info("archived"),
            "restored" => ctx.info("restored"),
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        let t = Theme::get();
        let attach_h = if self.attachments.is_empty() { 0 } else { 1 };
        let comp_h = (self.composer.height(area.width.saturating_sub(4)) + 2).clamp(3, 10);
        let [head, body, chips, composer] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(3), Constraint::Length(attach_h), Constraint::Length(comp_h)])
                .areas(area);
        f.render_widget(Paragraph::new(self.header()), head);

        let (feed_area, pane_area) = if self.show_pane && !self.pane.is_null() && body.width > 80 {
            let [a, b] = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(body);
            (a, Some(b))
        } else {
            (body, None)
        };
        let fb = block(
            if self.focus == Focus::Feed { "feed — j/k scroll · Enter reply · y/n approve" } else { "feed" },
            self.focus == Focus::Feed,
        );
        let inner = fb.inner(feed_area);
        let width = inner.width as usize;
        let key = (self.detail, inner.width, t);
        if self.feed_key != Some(key) {
            self.feed_cache.clear();
            self.feed_key = Some(key);
        }
        for (i, e) in self.events.iter().enumerate() {
            self.feed_cache
                .entry(event_key(e, i))
                .or_insert_with(|| event_lines(e, self.detail).into_iter().flat_map(|l| wrap(l, width)).collect());
        }
        let mut lines: Vec<Line> = vec![];
        if self.events.is_empty() {
            lines.push(Line::styled(
                if self.session.is_null() { "Loading…" } else { "No messages yet — write one below." },
                t.muted_style(),
            ));
        }
        if self.suggestion["suggest"] == true && self.session["projectId"].is_null() {
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                format!(
                    "  This chat has code ({}). Press P (in the feed) to move it to a project.",
                    self.suggestion["reason"].as_str().unwrap_or_default()
                ),
                t.accent_style(),
            ));
        }
        let extra: Vec<Line> = lines.into_iter().flat_map(|l| wrap(l, width)).collect();
        // only the visible rows are cloned
        let rows: Vec<&Line<'static>> = self
            .events
            .iter()
            .enumerate()
            .flat_map(|(i, e)| self.feed_cache.get(&event_key(e, i)).into_iter().flatten())
            .chain(extra.iter())
            .collect();
        self.scroll = self.scroll.min(rows.len());
        let visible: Vec<Line> = rows[tail_range(rows.len(), inner.height as usize, self.scroll)].iter().map(|l| (*l).clone()).collect();
        f.render_widget(Paragraph::new(visible).block(fb), feed_area);

        if let Some(pa) = pane_area {
            let title = format!(
                "{} · {}",
                self.pane["kind"].as_str().unwrap_or("pane"),
                self.pane["title"].as_str().or(self.pane["target"].as_str()).unwrap_or_default()
            );
            let pb = block(title, false);
            let pi = pb.inner(pa);
            let body: Vec<Line> = match self.pane["kind"].as_str() {
                Some("url") => {
                    let mut v = markdown::render(&format!("**{}**", self.pane["target"].as_str().unwrap_or_default()));
                    v.push(Line::styled("O (in the feed) opens it in your browser.", t.muted_style()));
                    v
                }
                _ => match &self.pane_body {
                    Some(lines) => lines.iter().take(pi.height as usize).cloned().collect(),
                    None => vec![Line::styled("Loading…", t.muted_style())],
                },
            };
            let shown: Vec<Line> = body.into_iter().take(pi.height as usize).collect();
            f.render_widget(Paragraph::new(shown).block(pb), pa);
        }

        if attach_h > 0 {
            let spans: Vec<Span> = self
                .attachments
                .iter()
                .map(|a| Span::styled(format!(" 📎 {} ", a["name"].as_str().unwrap_or_default()), t.accent_style()))
                .collect();
            f.render_widget(Paragraph::new(Line::from(spans)), chips);
        }
        let pending = self.pending().is_some();
        let hint = if pending {
            "agent asks permission — Ctrl+Y allow · Ctrl+N deny"
        } else if self.session["status"] == "running" {
            "agent is working — Ctrl+S stop · Enter queue a message"
        } else {
            "Message"
        };
        let cb = block(hint, self.focus == Focus::Composer);
        self.composer.draw(f, composer, cb, self.focus == Focus::Composer, "Ask the agent…");
    }
}

/// The feed cache key of an event: its uid (the position for an event without one).
fn event_key(e: &Value, index: usize) -> String {
    e["uid"].as_str().map(String::from).unwrap_or_else(|| format!("#{index}"))
}
