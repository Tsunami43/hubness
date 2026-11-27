//! The Sessions tab of a project or a group (the web app's SessionsTool): the scope's
//! sessions with search and the archive, and a new session in that scope.

use crossterm::event::{KeyCode, KeyEvent};
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use serde_json::{Value, json};

use crate::core::{Action, Ctx, Handled, Hit, Screen};
use crate::screens::composer::NewSession;
use crate::screens::home::route_of;
use crate::screens::kit::{click_keys, click_list, click_select, clicked_row};
use crate::ui::input::InputEvent;
use crate::ui::{Theme, ago, block};

/// Where the sessions live.
#[derive(Clone)]
pub enum Scope {
    Project(String),
    Group(String),
}

impl Scope {
    fn field(&self) -> &'static str {
        match self {
            Scope::Project(_) => "projectId",
            Scope::Group(_) => "groupId",
        }
    }

    fn id(&self) -> &str {
        match self {
            Scope::Project(id) | Scope::Group(id) => id,
        }
    }

    /// Whether session `s` belongs here.
    fn owns(&self, s: &Value) -> bool {
        s[self.field()].as_str() == Some(self.id())
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Focus {
    Composer,
    List,
}

pub struct SessionsTab {
    scope: Scope,
    new: NewSession,
    focus: Focus,
    list: ListState,
    filter: String,
    archive: bool,
    /// The scope's sessions from `GET /sessions?projectId=|groupId=` (kept live through the
    /// shared session list).
    loaded: Vec<Value>,
    archived: Vec<Value>,
}

impl SessionsTab {
    pub fn new(scope: Scope) -> Self {
        Self {
            scope,
            new: NewSession::default(),
            focus: Focus::List,
            list: ListState::default().with_selected(Some(0)),
            filter: String::new(),
            archive: false,
            loaded: vec![],
            archived: vec![],
        }
    }

    fn list_path(&self) -> String {
        format!("/sessions?{}={}", self.scope.field(), self.scope.id())
    }

    /// Active sessions: the live shared list (statuses, new ones) plus what the scope query
    /// returned and the shared list does not have yet.
    fn sessions(&self, ctx: &Ctx) -> Vec<Value> {
        let words: Vec<String> = self.filter.to_lowercase().split_whitespace().map(String::from).collect();
        let source: Vec<Value> = if self.archive {
            self.archived.iter().filter(|s| self.scope.owns(s)).cloned().collect()
        } else {
            let mut v: Vec<Value> = ctx.shared.sessions.iter().filter(|s| self.scope.owns(s)).cloned().collect();
            for s in &self.loaded {
                if s["archivedAt"].is_null() && !v.iter().any(|x| x["id"] == s["id"]) {
                    v.push(s.clone());
                }
            }
            v.sort_by(|a, b| b["updatedAt"].as_str().cmp(&a["updatedAt"].as_str()));
            v
        };
        source
            .into_iter()
            .filter(|s| {
                let hay = format!("{} {}", s["title"].as_str().unwrap_or_default(), s["agent"]["kind"].as_str().unwrap_or_default())
                    .to_lowercase();
                words.iter().all(|w| hay.contains(w))
            })
            .collect()
    }

    fn selected(&self, ctx: &Ctx) -> Option<Value> {
        self.list.selected().and_then(|i| self.sessions(ctx).get(i).cloned())
    }

    fn start(&mut self, ctx: &mut Ctx, prompt: &str) {
        let mut body = self.new.body(ctx, prompt);
        body[self.scope.field()] = json!(self.scope.id());
        ctx.send("started", Method::POST, "/sessions", body);
        self.new.input.clear();
    }

    fn reload(&self, ctx: &mut Ctx) {
        ctx.get("loaded", self.list_path());
        if self.archive {
            ctx.get("archived", "/sessions?archived=1");
        }
    }
}

impl Screen for SessionsTab {
    fn title(&self) -> String {
        "Sessions".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        self.reload(ctx);
    }

    fn captures_text(&self) -> bool {
        self.focus == Focus::Composer
    }

    fn actions(&self) -> Vec<Action> {
        let mut a = vec![Action::new("i", "New session: write the first message"), Action::new("N", "Start an empty session")];
        a.extend(NewSession::actions());
        a.extend([
            Action::new("/", "Search sessions"),
            Action::new("t", if self.archive { "Active sessions" } else { "Archive" }),
            Action::new("a", if self.archive { "Restore the selected" } else { "Archive the selected" }),
            Action::new("x", "Delete the selected for good"),
            Action::new("r", "Reload"),
        ]);
        a
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        if self.new.on_action(key, ctx) {
            return;
        }
        match key {
            "i" => self.focus = Focus::Composer,
            "N" => self.start(ctx, ""),
            "/" => ctx.prompt("filter", "Search sessions", self.filter.clone()),
            "t" => {
                self.archive = !self.archive;
                self.list.select(Some(0));
                self.reload(ctx);
            }
            "a" => {
                if let Some(s) = self.selected(ctx) {
                    let id = s["id"].as_str().unwrap_or_default();
                    let path = if self.archive { format!("/sessions/{id}/restore") } else { format!("/sessions/{id}/archive") };
                    ctx.send("changed", Method::POST, path, Value::Null);
                }
            }
            "x" => {
                if let Some(s) = self.selected(ctx) {
                    ctx.confirm(
                        &format!("delete:{}", s["id"].as_str().unwrap_or_default()),
                        "Delete the session",
                        format!("Delete “{}” for good?", s["title"].as_str().unwrap_or_default()),
                        true,
                    );
                }
            }
            "r" => self.reload(ctx),
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        if self.focus == Focus::Composer {
            return match self.new.input.on_key(k) {
                InputEvent::Submit => {
                    let prompt = self.new.input.value();
                    if !prompt.trim().is_empty() {
                        self.start(ctx, &prompt);
                    }
                    Handled::Yes
                }
                InputEvent::Cancel | InputEvent::Ignored if matches!(k.code, KeyCode::Esc | KeyCode::Tab | KeyCode::Down) => {
                    self.focus = Focus::List;
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
            KeyCode::Enter => {
                if let Some(s) = self.selected(ctx) {
                    ctx.go(route_of(&s));
                }
            }
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
            "loaded" => self.loaded = v.as_array().cloned().unwrap_or_default(),
            "archived" => self.archived = v.as_array().cloned().unwrap_or_default(),
            "filter" => {
                if let Some(f) = v.as_str() {
                    self.filter = f.to_string();
                    self.focus = Focus::List;
                    self.list.select(Some(0));
                }
            }
            "changed" => {
                let id = v["id"].clone();
                self.loaded.retain(|s| s["id"] != id);
                self.archived.retain(|s| s["id"] != id);
                ctx.shared.sessions.retain(|s| s["id"] != id);
                if v["archivedAt"].is_null() && v["id"].is_string() {
                    ctx.shared.sessions.insert(0, v);
                }
                self.reload(ctx);
            }
            k if k.starts_with("delete:") && v == Value::Bool(true) => {
                let id = k[7..].to_string();
                ctx.shared.sessions.retain(|s| s["id"] != id.as_str());
                self.loaded.retain(|s| s["id"] != id.as_str());
                ctx.send("changed", Method::DELETE, format!("/sessions/{id}"), Value::Null);
            }
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = Theme::get();
        let comp_h = (self.new.input.height(area.width.saturating_sub(4)) + 2).clamp(4, 8);
        let [settings, composer, list] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(comp_h), Constraint::Min(3)]).areas(area);
        let mut spans = self.new.settings_spans(ctx);
        spans.push(Span::styled(
            match self.scope {
                Scope::Project(_) => "the agent works in the project folder",
                Scope::Group(_) => "the agent sees every repository of the group",
            },
            t.muted_style(),
        ));
        click_keys(ctx, settings, &spans);
        f.render_widget(Paragraph::new(Line::from(spans)), settings);
        ctx.click(composer, Hit::Click("composer".into()));
        let focused = self.focus == Focus::Composer;
        self.new.input.draw(
            f,
            composer,
            block("new session — Enter start · Alt+Enter newline · Esc list · N empty session", focused),
            focused,
            "Give the agent a task…",
        );

        let sessions = self.sessions(ctx);
        let items: Vec<ListItem> = sessions
            .iter()
            .map(|s| {
                let (mark, color) = t.status(s["status"].as_str().unwrap_or("idle"));
                let agent = &s["agent"];
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {mark} "), Style::default().fg(color)),
                    Span::raw(s["title"].as_str().unwrap_or_default().to_string()),
                    Span::styled(
                        format!(
                            "   {}{} · {}",
                            agent["kind"].as_str().unwrap_or_default(),
                            agent["model"].as_str().map(|m| format!(" {m}")).unwrap_or_default(),
                            ago(s["updatedAt"].as_str().unwrap_or_default())
                        ),
                        t.muted_style(),
                    ),
                ]))
            })
            .collect();
        let title = match (self.archive, self.filter.is_empty()) {
            (true, _) => format!("archive ({}) — a restore · x delete · t back", sessions.len()),
            (false, true) => format!("sessions ({}) — Enter open · i new · / search · t archive", sessions.len()),
            (false, false) => format!("sessions matching “{}” ({})", self.filter, sessions.len()),
        };
        let focused = self.focus == Focus::List;
        if sessions.is_empty() {
            let hint =
                if self.archive { "The archive of this scope is empty." } else { "No sessions yet — press i and give the agent a task." };
            f.render_widget(Paragraph::new(Span::styled(hint, t.muted_style())).block(block(title, focused)), list);
            return;
        }
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
            if click_select(&mut self.list, i)
                && was
                && let Some(s) = self.selected(ctx)
            {
                ctx.go(route_of(&s));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_owns() {
        let p = Scope::Project("p1".into());
        assert!(p.owns(&json!({ "projectId": "p1" })));
        assert!(!p.owns(&json!({ "projectId": "p2" })));
        assert!(!Scope::Group("p1".into()).owns(&json!({ "projectId": "p1" })));
    }
}
