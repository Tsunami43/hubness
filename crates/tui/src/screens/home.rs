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

use super::composer::NewSession;
use super::kit::{click_keys, click_list, click_select, clicked_row};
use crate::core::{Action, Ctx, Handled, Hit, Route, Screen};
use crate::ui::input::InputEvent;
use crate::ui::{Theme, ago, block};

#[derive(PartialEq, Eq, Clone, Copy)]
enum Focus {
    Composer,
    List,
}

pub struct Home {
    new: NewSession,
    focus: Focus,
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
            new: NewSession::default(),
            // NORMAL first, like Vim: i starts typing
            focus: Focus::List,
            scope: None,
            list: ListState::default().with_selected(Some(0)),
            filter: String::new(),
            archive: false,
            archived: vec![],
        }
    }
}

impl Home {
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
        let prompt = self.new.input.value();
        if prompt.trim().is_empty() {
            return;
        }
        let mut body = self.new.body(ctx, &prompt);
        match &self.scope {
            Some((k, id)) if k == "project" => body["projectId"] = json!(id),
            Some((_, id)) => body["groupId"] = json!(id),
            None => {}
        }
        ctx.send("started", Method::POST, "/sessions", body);
        self.new.input.clear();
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
        let mut a = vec![Action::new("i", "Write the first message")];
        a.extend(NewSession::actions());
        a.extend([
            Action::new("w", "Where: project / group / chat"),
            Action::new("/", "Search sessions"),
            Action::new("t", if self.archive { "Active sessions" } else { "Archive" }),
            Action::new("a", if self.archive { "Restore the selected" } else { "Archive the selected" }),
            Action::new("x", "Delete the selected for good"),
        ]);
        a
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        if self.new.on_action(key, ctx) {
            return;
        }
        match key {
            "i" => self.focus = Focus::Composer,
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
            return match self.new.input.on_key(k) {
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
            _ if crate::screens::kit::list_motion(&mut self.list, &k, n) => {}
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn on_paste(&mut self, text: &str, _ctx: &mut Ctx) {
        self.focus = Focus::Composer;
        self.new.input.paste(text);
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        let Some(v) = ctx.ok(result) else { return };
        if self.new.on_task(key, &v, ctx) {
            return;
        }
        match key {
            "started" => {
                ctx.shared.sessions.insert(0, v.clone());
                ctx.go(route_of(&v));
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
        let comp_h = (self.new.input.height(area.width.saturating_sub(4)) + 2).clamp(4, 10);
        // a dashboard header (like LazyVim's) when there is room
        let logo_h = if area.height > 30 { crate::ui::LOGO_HEIGHT + 1 } else { 0 };
        let [logo, settings, composer, list] =
            Layout::vertical([Constraint::Length(logo_h), Constraint::Length(1), Constraint::Length(comp_h), Constraint::Min(3)])
                .areas(area);
        if logo_h > 0 {
            f.render_widget(crate::ui::logo(), logo);
        }
        let mut spans = self.new.settings_spans(ctx);
        spans.extend([Span::styled(" w ", t.key_style()), Span::raw(format!(" {}", self.scope_label(ctx)))]);
        click_keys(ctx, settings, &spans);
        f.render_widget(Paragraph::new(Line::from(spans)), settings);
        ctx.click(composer, Hit::Click("composer".into()));
        let hint = "new session — Enter start · Alt+Enter newline · Tab sessions";
        self.new.input.draw(
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
        let n = items.len();
        f.render_stateful_widget(List::new(items).block(block(title, focused)).highlight_style(t.selected()), list, &mut state);
        click_list(ctx, "list", list, true, &state, n);
    }

    fn on_click(&mut self, id: &str, ctx: &mut Ctx) {
        if id == "composer" {
            self.focus = Focus::Composer;
        } else if let Some(i) = clicked_row(id, "list") {
            let was = self.focus == Focus::List;
            self.focus = Focus::List;
            if click_select(&mut self.list, i) && was {
                self.open_selected(ctx);
            }
        }
    }
}
