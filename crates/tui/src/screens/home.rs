//! Chats (the web app's home page): start a new session — agent, model, permission mode and
//! where it runs (a project, a group or a plain chat) — and the session list with search and
//! the archive.

use crossterm::event::{KeyCode, KeyEvent};
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use serde_json::{Value, json};

use super::session::MODES;
use crate::core::{Action, Ctx, Handled, Route, Screen};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::{Theme, ago, block};

#[derive(PartialEq, Eq, Clone, Copy)]
enum Focus {
    Composer,
    List,
}

pub struct Home {
    composer: TextInput,
    focus: Focus,
    agent: Option<String>,
    model: Option<String>,
    mode: Option<String>,
    /// ("project" | "group", id) or None for a plain chat.
    scope: Option<(String, String)>,
    list: ListState,
    filter: String,
    archive: bool,
    archived: Vec<Value>,
}

impl Default for Home {
    fn default() -> Self {
        Self {
            composer: TextInput::multiline(),
            focus: Focus::Composer,
            agent: None,
            model: None,
            mode: None,
            scope: None,
            list: ListState::default(),
            filter: String::new(),
            archive: false,
            archived: vec![],
        }
    }
}

impl Home {
    fn agent(&self, ctx: &Ctx) -> String {
        self.agent.clone().unwrap_or_else(|| ctx.shared.agents["defaultKind"].as_str().unwrap_or("claude-code").to_string())
    }

    fn mode(&self, ctx: &Ctx) -> String {
        self.mode.clone().unwrap_or_else(|| ctx.shared.settings["defaultPermission"].as_str().unwrap_or("ask").to_string())
    }

    fn sessions<'a>(&'a self, ctx: &'a Ctx) -> Vec<&'a Value> {
        let words: Vec<String> = self.filter.to_lowercase().split_whitespace().map(String::from).collect();
        let source: &Vec<Value> = if self.archive { &self.archived } else { &ctx.shared.sessions };
        source.iter().filter(|s| words.iter().all(|w| s["title"].as_str().unwrap_or_default().to_lowercase().contains(w))).collect()
    }

    fn scope_label(&self, ctx: &Ctx) -> String {
        match &self.scope {
            Some((kind, id)) => {
                let list = if kind == "group" { &ctx.shared.groups } else { &ctx.shared.projects };
                let name = list.iter().find(|p| p["id"] == id.as_str()).and_then(|p| p["name"].as_str()).unwrap_or("?");
                format!("{kind} {name}")
            }
            None => "plain chat (Inbox)".into(),
        }
    }

    fn start(&mut self, ctx: &mut Ctx) {
        let prompt = self.composer.value();
        if prompt.trim().is_empty() {
            return;
        }
        let mut agent = json!({ "kind": self.agent(ctx), "permissionMode": self.mode(ctx) });
        if let Some(m) = &self.model {
            agent["model"] = json!(m);
        }
        let mut body = json!({ "agent": agent, "prompt": prompt });
        match &self.scope {
            Some((k, id)) if k == "project" => body["projectId"] = json!(id),
            Some((_, id)) => body["groupId"] = json!(id),
            None => {}
        }
        ctx.send("started", Method::POST, "/sessions", body);
        self.composer.clear();
    }

    fn open_selected(&mut self, ctx: &mut Ctx) {
        let Some(s) = self.list.selected().and_then(|i| self.sessions(ctx).get(i).map(|s| (*s).clone())) else { return };
        ctx.go(route_of(&s));
    }
}

/// Where a session opens: inside its project or group, or as a chat.
pub fn route_of(s: &Value) -> Route {
    let id = s["id"].as_str().unwrap_or_default().to_string();
    match (s["projectId"].as_str(), s["groupId"].as_str()) {
        (Some(p), _) => Route::ProjectSession { project: p.into(), session: id },
        (None, Some(g)) => Route::GroupSession { group: g.into(), session: id },
        _ => Route::Chat(id),
    }
}

impl Screen for Home {
    fn title(&self) -> String {
        "Chats".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        if self.archive {
            ctx.get("archived", "/sessions?archived=1");
        }
    }

    fn captures_text(&self) -> bool {
        self.focus == Focus::Composer
    }

    fn actions(&self) -> Vec<Action> {
        vec![
            Action::new("i", "Write the first message"),
            Action::new("A", "Agent"),
            Action::new("m", "Model"),
            Action::new("o", "Permission mode"),
            Action::new("w", "Where: project / group / chat"),
            Action::new("/", "Search sessions"),
            Action::new("t", if self.archive { "Active sessions" } else { "Archive" }),
            Action::new("a", if self.archive { "Restore the selected" } else { "Archive the selected" }),
            Action::new("x", "Delete the selected for good"),
        ]
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        match key {
            "i" => self.focus = Focus::Composer,
            "A" => {
                let items = ctx.shared.agents["agents"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|a| {
                        let ok = if a["available"] == true { "" } else { " (unavailable)" };
                        (
                            a["kind"].as_str().unwrap_or_default().to_string(),
                            format!("{} — {}{ok}", a["label"].as_str().unwrap_or_default(), a["detail"].as_str().unwrap_or_default()),
                        )
                    })
                    .collect();
                ctx.pick("agent", "Agent", items);
            }
            "m" => ctx.get("models", format!("/agents/{}/models", self.agent(ctx))),
            "o" => ctx.pick("mode", "Permission mode", MODES.iter().map(|(id, l)| (id.to_string(), l.to_string())).collect()),
            "w" => {
                let mut items = vec![("chat:".to_string(), "Plain chat (its own Inbox folder)".to_string())];
                items.extend(ctx.shared.projects.iter().filter(|p| p["kind"] == "normal").map(|p| {
                    (
                        format!("project:{}", p["id"].as_str().unwrap_or_default()),
                        format!("project {} — {}", p["name"].as_str().unwrap_or_default(), p["path"].as_str().unwrap_or_default()),
                    )
                }));
                items.extend(ctx.shared.groups.iter().map(|g| {
                    (format!("group:{}", g["id"].as_str().unwrap_or_default()), format!("group {}", g["name"].as_str().unwrap_or_default()))
                }));
                ctx.pick("scope", "Where should the agent work?", items);
            }
            "/" => ctx.prompt("filter", "Search sessions", self.filter.clone()),
            "t" => {
                self.archive = !self.archive;
                self.list.select(Some(0));
                if self.archive {
                    ctx.get("archived", "/sessions?archived=1");
                }
            }
            "a" => {
                if let Some(s) = self.list.selected().and_then(|i| self.sessions(ctx).get(i).map(|s| (*s).clone())) {
                    let id = s["id"].as_str().unwrap_or_default();
                    let path = if self.archive { format!("/sessions/{id}/restore") } else { format!("/sessions/{id}/archive") };
                    ctx.send("archived", Method::POST, path, Value::Null);
                }
            }
            "x" => {
                if let Some(s) = self.list.selected().and_then(|i| self.sessions(ctx).get(i).map(|s| (*s).clone())) {
                    ctx.confirm(
                        &format!("delete:{}", s["id"].as_str().unwrap_or_default()),
                        "Delete the session",
                        format!("Delete “{}” for good?", s["title"].as_str().unwrap_or_default()),
                        true,
                    );
                }
            }
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        if self.focus == Focus::Composer {
            return match self.composer.on_key(k) {
                InputEvent::Submit => {
                    self.start(ctx);
                    Handled::Yes
                }
                InputEvent::Cancel | InputEvent::Ignored if matches!(k.code, KeyCode::Esc | KeyCode::Tab | KeyCode::Down) => {
                    self.focus = Focus::List;
                    if self.list.selected().is_none() {
                        self.list.select(Some(0));
                    }
                    Handled::Yes
                }
                InputEvent::Changed => Handled::Yes,
                _ => Handled::No,
            };
        }
        let n = self.sessions(ctx).len();
        match k.code {
            KeyCode::Down | KeyCode::Char('j') if n > 0 => self.list.select(Some((self.list.selected().unwrap_or(0) + 1).min(n - 1))),
            KeyCode::Up | KeyCode::Char('k') => match self.list.selected() {
                Some(0) | None => self.focus = Focus::Composer,
                Some(i) => self.list.select(Some(i - 1)),
            },
            KeyCode::Enter => self.open_selected(ctx),
            KeyCode::Tab => self.focus = Focus::Composer,
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn on_paste(&mut self, text: &str, _ctx: &mut Ctx) {
        self.focus = Focus::Composer;
        self.composer.paste(text);
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        let Some(v) = ctx.ok(result) else { return };
        match key {
            "started" => {
                ctx.shared.sessions.insert(0, v.clone());
                ctx.go(route_of(&v));
            }
            "agent" => {
                if let Some(a) = v.as_str() {
                    self.agent = Some(a.to_string());
                    self.model = None;
                }
            }
            "models" => {
                let mut items: Vec<(String, String)> = vec![(String::new(), "Agent default".into())];
                match v["options"].as_array() {
                    Some(opts) => {
                        items.extend(opts.iter().map(|o| {
                            (o["id"].as_str().unwrap_or_default().to_string(), o["label"].as_str().unwrap_or_default().to_string())
                        }))
                    }
                    None => items.extend(
                        v["models"].as_array().into_iter().flatten().filter_map(|m| m.as_str()).map(|m| (m.to_string(), m.to_string())),
                    ),
                }
                ctx.pick("model", "Model", items);
            }
            "model" => {
                if let Some(m) = v.as_str() {
                    self.model = (!m.is_empty()).then(|| m.to_string());
                }
            }
            "mode" => {
                if let Some(m) = v.as_str() {
                    self.mode = Some(m.to_string());
                }
            }
            "scope" => {
                if let Some(s) = v.as_str() {
                    self.scope = s.split_once(':').filter(|(_, id)| !id.is_empty()).map(|(k, id)| (k.to_string(), id.to_string()));
                }
            }
            "filter" => {
                if let Some(f) = v.as_str() {
                    self.filter = f.to_string();
                    self.focus = Focus::List;
                    self.list.select(Some(0));
                }
            }
            "archived" => {
                if v.is_array() {
                    self.archived = v.as_array().cloned().unwrap_or_default();
                } else if self.archive {
                    ctx.get("archived", "/sessions?archived=1");
                }
            }
            k if k.starts_with("delete:") && v == Value::Bool(true) => {
                ctx.send("archived", Method::DELETE, format!("/sessions/{}", &k[7..]), Value::Null);
            }
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = Theme::get();
        let comp_h = (self.composer.height(area.width.saturating_sub(2)) + 2).clamp(4, 10);
        let [settings, composer, list] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(comp_h), Constraint::Min(3)]).areas(area);
        let model = self.model.clone().unwrap_or_else(|| "default model".into());
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" A ", t.key_style()),
                Span::raw(format!(" {}  ", self.agent(ctx))),
                Span::styled(" m ", t.key_style()),
                Span::raw(format!(" {model}  ")),
                Span::styled(" o ", t.key_style()),
                Span::raw(format!(" {}  ", self.mode(ctx))),
                Span::styled(" w ", t.key_style()),
                Span::raw(format!(" {}", self.scope_label(ctx))),
            ])),
            settings,
        );
        let hint = "new session — Enter start · Alt+Enter newline · Tab sessions";
        self.composer.draw(
            f,
            composer,
            block(hint, self.focus == Focus::Composer),
            self.focus == Focus::Composer,
            "What should the agent do?",
        );

        let sessions = self.sessions(ctx);
        let items: Vec<ListItem> = sessions
            .iter()
            .map(|s| {
                let (mark, color) = t.status(s["status"].as_str().unwrap_or("idle"));
                let scope = crate::app::session_label(s, &ctx.shared);
                let scope = scope.split("  · ").nth(1).unwrap_or_default().to_string();
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {mark} "), Style::default().fg(color)),
                    Span::raw(s["title"].as_str().unwrap_or_default().to_string()),
                    Span::styled(format!("   {scope} · {}", ago(s["updatedAt"].as_str().unwrap_or_default())), t.muted_style()),
                ]))
            })
            .collect();
        let title = match (self.archive, self.filter.is_empty()) {
            (true, _) => format!("archive ({}) — a restore · x delete · t back", sessions.len()),
            (false, true) => format!("sessions ({}) — Enter open · / search · t archive", sessions.len()),
            (false, false) => format!("sessions matching “{}” ({})", self.filter, sessions.len()),
        };
        let focused = self.focus == Focus::List;
        let mut state = self.list;
        if !focused {
            state.select(None);
        }
        f.render_stateful_widget(List::new(items).block(block(title, focused)).highlight_style(t.selected()), list, &mut state);
    }
}
