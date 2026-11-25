//! Settings (the web app's Settings pages): Agent (default permission mode, session and
//! project instructions), Git access (HTTPS logins per host), Users (hub accounts), and what is specific to the
//! TUI — the connection to the hub and the keyboard reference (the web app's appearance and
//! language settings belong to the browser).

use crossterm::event::{KeyCode, KeyEvent};
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{List, ListItem, Paragraph, Wrap};
use serde_json::{Value, json};

use super::kit::{Nav, Sections, row, s, since};
use super::session::{MODES, enc};
use super::tabs::Tab;
use crate::core::{Action, Ctx, Handled, Screen};
use crate::ui::{Theme, block};

pub fn screen(section: Option<&str>) -> Box<dyn Screen> {
    let tabs = vec![
        Tab::new("agent", "Agent", Box::new(AgentSettings::default())),
        Tab::new("git", "Git access", Box::new(GitAccess::default())),
        Tab::new("users", "Users", Box::new(Users::default())),
        Tab::new("connection", "Connection", Box::new(Connection::default())),
        Tab::new("keyboard", "Keyboard", Box::new(Keyboard::default())),
    ];
    Box::new(Sections::new("Settings", tabs, section))
}

/// What each permission mode means in practice (Settings → Agent).
fn mode_detail(id: &str) -> &'static str {
    match id {
        "ask" => "Every file edit and command waits for your approval in the feed.",
        "acceptEdits" => "Good for routine coding: edits go through, commands (installs, git, scripts) still ask.",
        "bypass" => "Nothing asks. In a project with the sandbox on, the agent is still limited to the project folder.",
        "plan" => "The agent first writes a plan without changing anything; you approve it, then it works.",
        _ => "",
    }
}

/// Wrap a paragraph of text into lines of `width` at word boundaries.
pub fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = vec![];
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push(line);
    }
    out
}

// ---------- Agent ----------

#[derive(Default)]
pub struct AgentSettings {
    nav: Nav,
}

impl AgentSettings {
    fn save(ctx: &mut Ctx, patch: Value) {
        ctx.send("saved", Method::PUT, "/settings", patch);
    }
}

impl Screen for AgentSettings {
    fn title(&self) -> String {
        "Agent".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        ctx.get("settings", "/settings");
    }

    fn actions(&self) -> Vec<Action> {
        vec![
            Action::new("p", "Default permission mode"),
            Action::new("e", "Edit the instructions for every session"),
            Action::new("n", "Edit the instructions for new projects"),
            Action::new("r", "Reload"),
        ]
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        let settings = ctx.shared.settings.clone();
        match key {
            "p" => {
                let current = s(&settings["defaultPermission"], "ask");
                let items =
                    MODES.iter().map(|(id, l)| (id.to_string(), format!("{}{l}", if *id == current { "✓ " } else { "  " }))).collect();
                ctx.pick("mode", "Default permission mode for new sessions", items);
            }
            "e" => ctx.prompt_multiline(
                "session-instructions",
                "Instructions for every session (Enter saves · Alt+Enter newline)",
                s(&settings["sessionInstructions"], ""),
            ),
            "n" => ctx.prompt_multiline(
                "default-instructions",
                "Instructions for new projects (Enter saves · Alt+Enter newline)",
                s(&settings["defaultInstructions"], ""),
            ),
            "r" => ctx.get("settings", "/settings"),
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        if self.nav.key(&k, MODES.len()) {
            return Handled::Yes;
        }
        if k.code == KeyCode::Enter
            && let Some((id, _)) = self.nav.selected().and_then(|i| MODES.get(i))
        {
            if ctx.shared.settings["defaultPermission"] != *id {
                Self::save(ctx, json!({ "defaultPermission": id }));
            }
            return Handled::Yes;
        }
        Handled::No
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        match key {
            "settings" => {
                if let Some(v) = ctx.ok(result) {
                    ctx.shared.settings = v;
                }
            }
            "saved" => {
                if let Some(v) = ctx.ok(result) {
                    ctx.shared.settings = v;
                    ctx.info("saved");
                }
            }
            "mode" => {
                if let Ok(Value::String(m)) = result {
                    Self::save(ctx, json!({ "defaultPermission": m }));
                }
            }
            "session-instructions" => {
                if let Ok(Value::String(text)) = result {
                    Self::save(ctx, json!({ "sessionInstructions": text }));
                }
            }
            "default-instructions" => {
                if let Ok(Value::String(text)) = result {
                    Self::save(ctx, json!({ "defaultInstructions": text }));
                }
            }
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = Theme::get();
        let st = &ctx.shared.settings;
        let current = s(&st["defaultPermission"], "ask");
        let w = area.width.saturating_sub(6) as usize;
        let bypass = current == "bypass";
        let [modes, warn, session, project] = Layout::vertical([
            Constraint::Length(MODES.len() as u16 * 3 + 4),
            Constraint::Length(if bypass { 2 } else { 0 }),
            Constraint::Min(5),
            Constraint::Min(5),
        ])
        .areas(area);

        // permission modes
        let b = block("Default permission mode — ↑↓ Enter pick · p picker", true);
        let inner = b.inner(modes);
        f.render_widget(b, modes);
        let [hint, list] = Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(inner);
        f.render_widget(
            Paragraph::new(
                "Every new session starts with this mode. You can still change it for a single session (o in a chat). Applies to all harnesses.",
            )
            .style(t.muted_style())
            .wrap(Wrap { trim: true }),
            hint,
        );
        let items: Vec<ListItem> = MODES
            .iter()
            .map(|(id, label)| {
                let on = *id == current;
                let mut head = vec![
                    Span::styled(if on { " ✓ " } else { "   " }, t.accent_style()),
                    Span::styled(label.to_string(), if *id == "bypass" { Style::default().fg(t.warn) } else { t.bold() }),
                ];
                if *id == "ask" {
                    head.push(Span::styled("  recommended", t.muted_style()));
                }
                let detail: String = mode_detail(id).chars().take(w).collect();
                ListItem::new(Text::from(vec![Line::from(head), Line::styled(format!("   {detail}"), t.muted_style()), Line::raw("")]))
            })
            .collect();
        self.nav.clamp(MODES.len());
        f.render_stateful_widget(List::new(items).highlight_style(t.selected()), list, &mut self.nav.state);
        if bypass {
            f.render_widget(
                Paragraph::new(
                    " ⚠ New sessions will run commands without asking. Keep the sandbox on in project settings and watch the feed.",
                )
                .style(Style::default().fg(t.warn))
                .wrap(Wrap { trim: true }),
                warn,
            );
        }

        // instructions
        for (area, key, title, hint, field, placeholder) in [
            (
                session,
                "e",
                "Instructions for every session",
                "Added to every agent session — in projects, groups and chats. Changes apply to sessions started after saving; project and group instructions win when they conflict.",
                "sessionInstructions",
                "(none) — e.g. “Answer in English. Prefer small commits.”",
            ),
            (
                project,
                "n",
                "Instructions for new projects",
                "Copied into the AI agent instructions of every project created from now on (new, cloned, imported, made from a chat). Each project can change its own copy in its Settings; existing projects are not touched.",
                "defaultInstructions",
                "(none)",
            ),
        ] {
            let b = block(format!("{title} — {key} edit"), false);
            let inner = b.inner(area);
            f.render_widget(b, area);
            let mut lines: Vec<Line> =
                wrap_words(hint, inner.width as usize).into_iter().map(|l| Line::styled(l, t.muted_style())).collect();
            let text = s(&st[field], "");
            if text.trim().is_empty() {
                lines.push(Line::styled(placeholder, t.muted_style()));
            } else {
                lines.extend(text.lines().map(|l| Line::raw(l.to_string())));
            }
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
    }
}

// ---------- Git access ----------

/// Where to create a personal access token on known hostings.
pub fn token_page(host: &str) -> Option<String> {
    if host == "github.com" || host.starts_with("github.") {
        Some(format!("https://{host}/settings/tokens"))
    } else if host == "gitlab.com" || host.starts_with("gitlab.") {
        Some(format!("https://{host}/-/user_settings/personal_access_tokens"))
    } else {
        None
    }
}

/// A login being entered: host, kind, username (the secret is asked last).
#[derive(Default, Clone)]
struct Draft {
    host: String,
    kind: String,
    username: String,
}

#[derive(Default)]
pub struct GitAccess {
    list: Option<Vec<Value>>,
    error: Option<String>,
    nav: Nav,
    draft: Option<Draft>,
}

impl GitAccess {
    fn selected(&self) -> Option<Value> {
        self.nav.selected().and_then(|i| self.list.as_ref()?.get(i).cloned())
    }

    fn ask_kind(&self, ctx: &mut Ctx) {
        let Some(d) = &self.draft else { return };
        let items = vec![
            ("token".to_string(), "Token (recommended) — a personal access token with repository access".to_string()),
            ("password".to_string(), "Login + password — many hostings refuse passwords for git".to_string()),
        ];
        ctx.pick("kind", format!("How to sign in to {}", d.host), items);
    }

    fn edit(&mut self, c: &Value, ctx: &mut Ctx) {
        self.draft =
            Some(Draft { host: s(&c["host"], "").into(), kind: s(&c["kind"], "token").into(), username: s(&c["username"], "").into() });
        self.ask_kind(ctx);
    }
}

impl Screen for GitAccess {
    fn title(&self) -> String {
        "Git access".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        ctx.get("list", "/git-credentials");
    }

    fn actions(&self) -> Vec<Action> {
        let mut a = vec![Action::new("a", "Add a login"), Action::new("r", "Reload")];
        if let Some(c) = self.selected() {
            a.push(Action::new("e", "Edit the selected login"));
            a.push(Action::new("x", "Delete the selected login"));
            if token_page(s(&c["host"], "")).is_some() {
                a.push(Action::new("t", "Open the page to create a token"));
            }
        }
        a
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        match key {
            "a" => {
                self.draft = Some(Draft { kind: "token".into(), ..Default::default() });
                ctx.prompt("host", "Host (e.g. gitlab.example.com — the login is used for every repository on it)", "");
            }
            "r" => ctx.get("list", "/git-credentials"),
            "e" => {
                if let Some(c) = self.selected() {
                    self.edit(&c, ctx);
                }
            }
            "x" => {
                if let Some(c) = self.selected() {
                    let host = s(&c["host"], "").to_string();
                    ctx.confirm(&format!("delete:{host}"), "Delete the login", format!("Delete the login for {host}?"), true);
                }
            }
            "t" => {
                if let Some(url) = self.selected().and_then(|c| token_page(s(&c["host"], ""))) {
                    ctx.open_url(url);
                }
            }
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        let n = self.list.as_ref().map_or(0, |l| l.len());
        if self.nav.key(&k, n) {
            return Handled::Yes;
        }
        if k.code == KeyCode::Enter {
            if let Some(c) = self.selected() {
                self.edit(&c, ctx);
            }
            return Handled::Yes;
        }
        Handled::No
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        match key {
            "list" | "saved" => match result {
                Ok(v) => {
                    if key == "saved" {
                        ctx.info("login saved");
                    }
                    self.list = Some(v.as_array().cloned().unwrap_or_default());
                    self.error = None;
                }
                Err(e) => self.error = Some(e.to_string()),
            },
            "host" => match (result, &mut self.draft) {
                (Ok(Value::String(h)), Some(d)) if !h.trim().is_empty() => {
                    d.host = h.trim().to_lowercase();
                    // a login already stored for the host: prefill it
                    if let Some(c) = self.list.iter().flatten().find(|c| c["host"] == d.host.as_str()) {
                        d.kind = s(&c["kind"], "token").into();
                        d.username = s(&c["username"], "").into();
                    }
                    self.ask_kind(ctx);
                }
                _ => self.draft = None,
            },
            "kind" => match (result, &mut self.draft) {
                (Ok(Value::String(k)), Some(d)) => {
                    d.kind = k;
                    let title = format!("Username on {}", d.host);
                    let value = d.username.clone();
                    ctx.prompt("username", title, value);
                }
                _ => self.draft = None,
            },
            "username" => match (result, &mut self.draft) {
                (Ok(Value::String(u)), Some(d)) if !u.trim().is_empty() => {
                    d.username = u.trim().to_string();
                    let what = if d.kind == "token" { "Token" } else { "Password" };
                    let page = if d.kind == "token" {
                        token_page(&d.host).map(|_| " (t on the list opens the token page)".to_string()).unwrap_or_default()
                    } else {
                        String::new()
                    };
                    ctx.prompt_secret("secret", format!("{what} for {}@{}{page}", d.username, d.host));
                }
                _ => self.draft = None,
            },
            "secret" => {
                let draft = self.draft.take();
                if let (Ok(Value::String(secret)), Some(d)) = (result, draft)
                    && !secret.is_empty()
                {
                    ctx.send(
                        "saved",
                        Method::PUT,
                        format!("/git-credentials/{}", enc(&d.host)),
                        json!({ "username": d.username, "kind": d.kind, "secret": secret }),
                    );
                }
            }
            k if k.starts_with("delete:") => {
                if matches!(result, Ok(Value::Bool(true))) {
                    ctx.send("list", Method::DELETE, format!("/git-credentials/{}", enc(&k[7..])), Value::Null);
                }
            }
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        let t = Theme::get();
        let [hint, body] = Layout::vertical([Constraint::Length(3), Constraint::Min(3)]).areas(area);
        let mut lines: Vec<Line> = wrap_words(
            "Logins the hub uses for fetch, pull, push and clone over HTTPS. Stored encrypted on the server and never shown again; agents don’t get them.",
            area.width.saturating_sub(2) as usize,
        )
        .into_iter()
        .map(|l| Line::styled(format!(" {l}"), t.muted_style()))
        .collect();
        if let Some(e) = &self.error {
            lines.push(Line::styled(format!(" {e}"), Style::default().fg(t.danger)));
        }
        f.render_widget(Paragraph::new(lines), hint);
        let title = "logins — a add · Enter/e edit · x delete · t token page";
        let Some(list) = &self.list else {
            f.render_widget(Paragraph::new("Loading…").block(block(title, true)), body);
            return;
        };
        if list.is_empty() {
            f.render_widget(
                Paragraph::new(Line::styled("No logins yet — a adds one (a token is recommended).", t.muted_style()))
                    .block(block(title, true)),
                body,
            );
            return;
        }
        let items: Vec<ListItem> = list
            .iter()
            .map(|c| {
                let kind = if c["kind"] == "token" { "token" } else { "password" };
                ListItem::new(Text::from(vec![
                    Line::from(vec![Span::styled(" ⚿ ", t.accent_style()), Span::styled(s(&c["host"], "").to_string(), t.bold())]),
                    Line::styled(
                        format!("   {} · {kind} · updated {}", s(&c["username"], ""), since(s(&c["updatedAt"], ""))),
                        t.muted_style(),
                    ),
                ]))
            })
            .collect();
        self.nav.clamp(list.len());
        f.render_stateful_widget(List::new(items).block(block(title, true)).highlight_style(t.selected()), body, &mut self.nav.state);
    }
}

// ---------- Users ----------

/// A password change in progress: whose, and the current password when it is your own.
#[derive(Clone)]
struct PasswordChange {
    id: String,
    username: String,
    current: Option<String>,
}

/// Hub accounts (Settings → Users): everyone has the same rights.
#[derive(Default)]
pub struct Users {
    list: Option<Vec<Value>>,
    error: Option<String>,
    nav: Nav,
    /// The signed-in user's id (`None` on a hub without accounts).
    me: Option<String>,
    /// The username of an account being added (the password is asked next).
    adding: Option<String>,
    change: Option<PasswordChange>,
    /// The new password, until it is typed again.
    new_password: Option<String>,
}

impl Users {
    fn selected(&self) -> Option<Value> {
        self.nav.selected().and_then(|i| self.list.as_ref()?.get(i).cloned())
    }

    fn reload(&self, ctx: &Ctx) {
        ctx.get("list", "/users");
        ctx.get("me", "/auth/status");
    }

    fn change_password(&mut self, u: &Value, ctx: &mut Ctx) {
        let (id, username) = (s(&u["id"], "").to_string(), s(&u["username"], "").to_string());
        let own = self.me.as_deref() == Some(id.as_str());
        self.change = Some(PasswordChange { id, username: username.clone(), current: None });
        if own {
            ctx.prompt_secret("current", "Your current password");
        } else {
            ctx.prompt_secret("new-password", format!("New password for {username} (at least 8 characters)"));
        }
    }
}

impl Screen for Users {
    fn title(&self) -> String {
        "Users".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        self.reload(ctx);
    }

    fn actions(&self) -> Vec<Action> {
        let mut a = vec![Action::new("a", "Add an account"), Action::new("r", "Reload")];
        if self.selected().is_some() {
            a.push(Action::new("p", "Change the password of the selected account"));
            a.push(Action::new("x", "Delete the selected account"));
        }
        a
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        match key {
            "a" => ctx.prompt("username", "Username of the new account (letters, digits, . _ -)", ""),
            "r" => self.reload(ctx),
            "p" => {
                if let Some(u) = self.selected() {
                    self.change_password(&u, ctx);
                }
            }
            "x" => {
                if let Some(u) = self.selected() {
                    let name = s(&u["username"], "");
                    let own = self.me.as_deref() == u["id"].as_str();
                    let msg = if own {
                        format!("Delete your own account {name}? You will be signed out.")
                    } else {
                        format!("Delete the account {name}? Its sessions end right away.")
                    };
                    ctx.confirm(&format!("delete:{}", s(&u["id"], "")), "Delete the account", msg, true);
                }
            }
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        let n = self.list.as_ref().map_or(0, |l| l.len());
        if self.nav.key(&k, n) {
            return Handled::Yes;
        }
        if k.code == KeyCode::Enter {
            if let Some(u) = self.selected() {
                self.change_password(&u, ctx);
            }
            return Handled::Yes;
        }
        Handled::No
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        match key {
            "list" => match result {
                Ok(v) => {
                    self.list = Some(v.as_array().cloned().unwrap_or_default());
                    self.error = None;
                }
                Err(e) => self.error = Some(e.to_string()),
            },
            "me" => {
                if let Ok(v) = result {
                    self.me = v["user"]["id"].as_str().map(String::from);
                }
            }
            "username" => {
                if let Ok(Value::String(name)) = result
                    && !name.trim().is_empty()
                {
                    let name = name.trim().to_string();
                    ctx.prompt_secret("password", format!("Password for {name} (at least 8 characters)"));
                    self.adding = Some(name);
                }
            }
            "password" => {
                if let (Ok(Value::String(password)), Some(name)) = (result, self.adding.take())
                    && !password.is_empty()
                {
                    ctx.send("added", Method::POST, "/users", json!({ "username": name, "password": password }));
                }
            }
            "added" => {
                if let Some(u) = ctx.ok(result) {
                    ctx.info(format!("added {}", s(&u["username"], "")));
                    self.reload(ctx);
                }
            }
            "current" => match (result, &mut self.change) {
                (Ok(Value::String(current)), Some(c)) if !current.is_empty() => {
                    c.current = Some(current);
                    ctx.prompt_secret("new-password", "Your new password (at least 8 characters)");
                }
                _ => self.change = None,
            },
            "new-password" => match (result, &self.change) {
                (Ok(Value::String(password)), Some(c)) if !password.is_empty() => {
                    ctx.prompt_secret("repeat-password", format!("Repeat the new password for {}", c.username));
                    self.new_password = Some(password);
                }
                _ => self.change = None,
            },
            "repeat-password" => {
                let (change, first) = (self.change.take(), self.new_password.take());
                if let (Ok(Value::String(again)), Some(c), Some(first)) = (result, change, first) {
                    if again != first {
                        ctx.error("the passwords don't match");
                        return;
                    }
                    let mut body = json!({ "password": first });
                    if let Some(cur) = c.current {
                        body["currentPassword"] = json!(cur);
                    }
                    ctx.send(format!("changed:{}", c.username), Method::PUT, format!("/users/{}/password", enc(&c.id)), body);
                }
            }
            k if k.starts_with("changed:") => {
                if ctx.ok(result).is_some() {
                    ctx.info(format!("password changed for {} (its other sessions ended)", &k[8..]));
                }
            }
            k if k.starts_with("delete:") => {
                if matches!(result, Ok(Value::Bool(true))) {
                    ctx.send("deleted", Method::DELETE, format!("/users/{}", enc(&k[7..])), Value::Null);
                }
            }
            "deleted" => {
                // deleting your own account ends this session: the sign-in screen follows
                let deleted = ctx.ok(result).is_some();
                if deleted {
                    ctx.info("account deleted");
                    self.reload(ctx);
                }
            }
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        let t = Theme::get();
        let [hint, body] = Layout::vertical([Constraint::Length(4), Constraint::Min(3)]).areas(area);
        let open = self.list.as_ref().is_some_and(|l| l.is_empty());
        let text = if open {
            "No accounts yet: the hub is open to this machine only. The first account turns sign-in on — for the web app, this TUI and every other client."
        } else {
            "Everyone with an account has the same rights. A password change or a deleted account ends its sessions."
        };
        let mut lines: Vec<Line> = wrap_words(text, area.width.saturating_sub(2) as usize)
            .into_iter()
            .map(|l| Line::styled(format!(" {l}"), if open { Style::default().fg(t.warn) } else { t.muted_style() }))
            .collect();
        if let Some(e) = &self.error {
            lines.push(Line::styled(format!(" {e}"), Style::default().fg(t.danger)));
        }
        f.render_widget(Paragraph::new(lines), hint);
        let title = "accounts — a add · Enter/p password · x delete";
        let Some(list) = &self.list else {
            f.render_widget(Paragraph::new("Loading…").block(block(title, true)), body);
            return;
        };
        if list.is_empty() {
            f.render_widget(
                Paragraph::new(Line::styled("No accounts — a adds the first one.", t.muted_style())).block(block(title, true)),
                body,
            );
            return;
        }
        let items: Vec<ListItem> = list
            .iter()
            .map(|u| {
                let me = self.me.as_deref() == u["id"].as_str();
                let mut head = vec![
                    Span::styled(format!(" {} ", crate::ui::theme::icon("\u{f0004}", "@")), t.accent_style()),
                    Span::styled(s(&u["username"], "").to_string(), t.bold()),
                ];
                if me {
                    head.push(Span::styled("  you", Style::default().fg(t.ok)));
                }
                ListItem::new(Text::from(vec![
                    Line::from(head),
                    Line::styled(format!("   created {}", since(s(&u["createdAt"], ""))), t.muted_style()),
                ]))
            })
            .collect();
        self.nav.clamp(list.len());
        f.render_stateful_widget(List::new(items).block(block(title, true)).highlight_style(t.selected()), body, &mut self.nav.state);
    }
}

// ---------- Connection ----------

#[derive(Default)]
pub struct Connection {
    auth: Value,
    health: Option<Result<u128, String>>,
    /// Saved hubs (`~/.config/hubness/tui.json`).
    hubs: Vec<crate::hubs::Hub>,
    nav: Nav,
    /// The URL of the hub this TUI is on.
    current: String,
    /// `/settings` of the hub: its data and projects folders.
    settings: Value,
}

impl Connection {
    fn selected_hub(&self) -> Option<String> {
        self.nav.selected().and_then(|i| self.hubs.get(i)).map(|h| h.url.clone())
    }

    fn check(&mut self, ctx: &Ctx) {
        self.hubs = crate::hubs::Store::load().hubs;
        self.current = ctx.client.base().to_string();
        ctx.get("auth", "/auth/status");
        ctx.get("settings", "/settings");
        let c = ctx.client.clone();
        ctx.task("health", async move {
            let start = std::time::Instant::now();
            c.health().await?;
            Ok(json!(start.elapsed().as_millis() as u64))
        });
    }
}

impl Screen for Connection {
    fn title(&self) -> String {
        "Connection".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        self.check(ctx);
    }

    fn actions(&self) -> Vec<Action> {
        let mut a = vec![Action::new("r", "Check again"), Action::new("o", "Open the hub in the browser"), Action::new("a", "Add a hub")];
        if let Some(url) = self.selected_hub()
            && url != self.current
        {
            a.push(Action::new("s", "Switch to the selected hub"));
        }
        if self.selected_hub().is_some() {
            a.push(Action::new("x", "Forget the selected hub"));
        }
        a
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        match key {
            "r" => self.check(ctx),
            "a" => ctx.switch_hub(None),
            "s" => {
                if let Some(url) = self.selected_hub() {
                    ctx.switch_hub(Some(url));
                }
            }
            "x" => {
                if let Some(url) = self.selected_hub() {
                    if url == ctx.client.base() {
                        ctx.error("This is the hub you are on: switch to another one first");
                    } else {
                        ctx.confirm(&format!("forget:{url}"), "Forget the hub", format!("Forget {url} and its saved sign-in?"), true);
                    }
                }
            }
            "o" => {
                let url = ctx.shared.base_url.clone();
                ctx.open_url(url);
            }
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        if self.nav.key(&k, self.hubs.len()) {
            return Handled::Yes;
        }
        if k.code == KeyCode::Enter
            && let Some(url) = self.selected_hub()
        {
            ctx.switch_hub(Some(url));
            return Handled::Yes;
        }
        Handled::No
    }

    fn on_click(&mut self, id: &str, ctx: &mut Ctx) {
        if let Some(i) = id.strip_prefix("hubs:").and_then(|i| i.parse::<usize>().ok()) {
            if self.nav.selected() == Some(i) {
                if let Some(url) = self.selected_hub() {
                    ctx.switch_hub(Some(url));
                }
            } else {
                self.nav.state.select(Some(i));
            }
        }
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        match key {
            k if k.starts_with("forget:") => {
                if matches!(result, Ok(Value::Bool(true))) {
                    let mut store = crate::hubs::Store::load();
                    store.forget(&k[7..]);
                    store.save();
                    self.hubs = store.hubs;
                    ctx.info(format!("forgot {}", &k[7..]));
                }
            }
            "auth" => self.auth = result.unwrap_or_else(|e| json!({ "error": e.to_string() })),
            "settings" => self.settings = result.unwrap_or(Value::Null),
            "health" => self.health = Some(result.map(|v| v.as_u64().unwrap_or(0) as u128).map_err(|e| e.to_string())),
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = Theme::get();
        const W: usize = 20;
        let health = match &self.health {
            None => "checking…".to_string(),
            Some(Ok(ms)) => format!("reachable ({ms} ms)"),
            Some(Err(e)) => format!("not reachable: {e}"),
        };
        let sign_in = if self.auth.is_null() {
            "checking…".to_string()
        } else if let Some(e) = self.auth["error"].as_str() {
            e.to_string()
        } else if self.auth["required"] == true {
            match self.auth["user"]["username"].as_str() {
                Some(u) => format!("required — signed in as {u} (Space L logs out)"),
                None => "required — this TUI is NOT signed in".into(),
            }
        } else {
            "not required (no accounts yet: open to the hub's machine only)".into()
        };
        let agents = ctx.shared.agents["agents"].as_array().map_or(0, |a| a.len());
        let lines = vec![
            Line::styled("Hub", t.accent_style()),
            row("URL", ctx.shared.base_url.clone(), W),
            row("Status", health, W),
            row("Sign-in", sign_in, W),
            row("Live sessions", ctx.shared.sessions.len().to_string(), W),
            row("Projects / groups", format!("{} / {}", ctx.shared.projects.len(), ctx.shared.groups.len()), W),
            row("Agents", agents.to_string(), W),
            row("Data folder", self.settings["dataDir"].as_str().unwrap_or("…").to_string(), W),
            row("Projects folder", self.settings["projectsRoot"].as_str().unwrap_or("…").to_string(), W),
            row("Change on the hub", "hubcli server config data-dir <path> · then hubcli server restart", W),
            Line::raw(""),
            Line::styled("This app", t.accent_style()),
            row("hubness TUI", format!("v{}", env!("CARGO_PKG_VERSION")), W),
            row("Other hubs", "Space H switches; a adds one (hubtui --url <address> too)", W),
            Line::raw(""),
            Line::styled(
                "  The TUI is a client of the hub's API: every session, project and setting is shared with the web app.",
                t.muted_style(),
            ),
            Line::styled(
                "  Appearance and language of the web app are stored in the browser; the TUI takes its colors from the active Omarchy theme.",
                t.muted_style(),
            ),
        ];
        let list_h = (self.hubs.len() as u16 + 2).clamp(3, 10);
        let [info, list] = Layout::vertical([Constraint::Min(5), Constraint::Length(list_h)]).areas(area);
        f.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }).block(block("Connection — r check again · o open in the browser", false)),
            info,
        );
        let current = ctx.shared.base_url.trim_end_matches('/');
        let items: Vec<ListItem> = self
            .hubs
            .iter()
            .map(|h| {
                let mut spans = vec![
                    Span::styled(format!(" {} ", crate::ui::theme::icon("\u{f048b}", "-")), t.accent_style()),
                    Span::styled(h.url.clone(), t.bold()),
                ];
                if let Some(u) = &h.username {
                    spans.push(Span::styled(format!("  {u}"), t.muted_style()));
                }
                if h.url == current {
                    spans.push(Span::styled("  current", Style::default().fg(t.ok)));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();
        let b = block("saved hubs — Enter/s switch · a add · x forget", true);
        ctx.click_rows("hubs", b.inner(list), self.nav.state.offset(), items.len());
        self.nav.clamp(items.len());
        f.render_stateful_widget(List::new(items).block(b).highlight_style(t.selected()), list, &mut self.nav.state);
    }
}

// ---------- Keyboard ----------

/// The keyboard reference: global keys, then per-screen keys.
pub const KEYS: &[(&str, &[(&str, &str)])] = &[
    (
        "Modes (like Vim)",
        &[
            ("NORMAL", "browse: motions and single-letter actions"),
            ("INSERT", "typing in a box, prompt, filter or terminal — Esc back to NORMAL"),
            (":", "command palette: every action of the screen and global commands"),
            ("?", "which-key: the keys of this screen"),
        ],
    ),
    (
        "Motions (NORMAL)",
        &[
            ("j k", "down / up (counts work: 5j)"),
            ("h l", "left / right: fold, unfold, out of / into a folder"),
            ("gg G", "first / last"),
            ("Ctrl+d Ctrl+u", "half a page down / up"),
            ("Ctrl+f Ctrl+b", "a page down / up"),
            ("Ctrl+h Ctrl+l", "focus the pane to the left / right (also Ctrl+w h/j/k/l, Space w h/j/k/l)"),
            ("gt gT", "next / previous tab (also Alt+1…9)"),
            ("Enter", "open / confirm"),
            ("/", "filter or search the list"),
            ("i", "start typing (Enter in a feed)"),
            ("q Esc", "back (q quits on the first screen)"),
        ],
    ),
    (
        "Leader: Space (Ctrl+G while typing)",
        &[
            ("Space", "jump to a session"),
            ("c p s h ,", "chats · projects · system · harness · settings"),
            ("a", "+actions: the keys of this screen (also ?)"),
            ("w h/j/k/l", "+windows: focus another pane (w cycles)"),
            ("t n/p/1…9", "+tabs: next / previous / a tab by name"),
            ("e", "toggle the sidebar"),
            ("r", "reload"),
            ("H", "switch hub: the saved hubs, or add one"),
            ("L", "log out (a hub with accounts)"),
            ("q", "quit (also Ctrl+Q)"),
            ("Backspace", "up a menu (Esc closes)"),
        ],
    ),
    (
        "File editor (Files tab)",
        &[
            ("e", "edit the open file (Vim NORMAL, shown as EDIT)"),
            ("i a o", "start typing; Esc stops"),
            (":w Ctrl+S", "save"),
            (":q Esc", "close (:q! discards the edits)"),
        ],
    ),
    (
        "Mouse",
        &[
            ("click", "sidebar, tabs, key pills, list rows, status-line hints, which-key"),
            ("click again", "open the selected row"),
            ("wheel", "scroll the list or viewer under the pointer"),
            ("Shift+drag", "select text in the terminal (HUBNESS_TUI_MOUSE=0 turns the mouse off)"),
        ],
    ),
    (
        "Chats and sessions",
        &[
            ("Enter", "Send (Alt+Enter newline)"),
            ("Tab / Esc", "Between the message box and the list or feed"),
            ("Ctrl+Y / Ctrl+N", "Approve / deny a pending request"),
            ("Ctrl+S", "Stop the agent"),
            ("m / o", "Model / permission mode"),
        ],
    ),
    (
        "System",
        &[
            ("Enter / l", "Logs of a container or unit (a asks the agent about them)"),
            ("f / u", "Show running / stopped / all · scope system / user"),
            ("s S R", "Start / stop / restart"),
            ("x", "Remove (stopped containers, unused networks and volumes)"),
            ("[ ] / m", "Package manager"),
        ],
    ),
    (
        "Harness",
        &[
            ("r", "Re-check version, update and sign-in"),
            ("i U l o", "Install / update / sign in / sign out (in a terminal)"),
            ("m", "Default model"),
            ("c", "Copy a command"),
        ],
    ),
    ("Prompts and pickers", &[("Enter", "Confirm"), ("Esc", "Cancel"), ("type", "Filter a picker"), ("y / n", "Answer a confirmation")]),
    (
        "Terminals (jobs, shells)",
        &[("keys", "go to the terminal while it runs"), ("Ctrl+G w", "Close the terminal"), ("Esc", "Close once it has exited")],
    ),
];

#[derive(Default)]
pub struct Keyboard {
    scroll: u16,
}

impl Screen for Keyboard {
    fn title(&self) -> String {
        "Keyboard".into()
    }

    fn on_key(&mut self, k: KeyEvent, _ctx: &mut Ctx) -> Handled {
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Home | KeyCode::Char('g') => self.scroll = 0,
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        let t = Theme::get();
        let mut lines = vec![];
        for (title, keys) in KEYS {
            lines.push(Line::styled(title.to_string(), t.accent_style()));
            for (k, label) in keys.iter() {
                lines.push(Line::from(vec![Span::styled(format!("  {k:<16}"), t.bold()), Span::raw(label.to_string())]));
            }
            lines.push(Line::raw(""));
        }
        let max = (lines.len() as u16).saturating_sub(area.height.saturating_sub(2));
        self.scroll = self.scroll.min(max);
        f.render_widget(
            Paragraph::new(lines)
                .scroll((self.scroll, 0))
                .block(block("Keyboard — the leader panel always lists the current screen's keys", false)),
            area,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_pages() {
        assert_eq!(token_page("github.com").as_deref(), Some("https://github.com/settings/tokens"));
        assert_eq!(token_page("code.example.com"), None);
        assert_eq!(token_page("gitlab.com").as_deref(), Some("https://gitlab.com/-/user_settings/personal_access_tokens"));
    }

    #[test]
    fn word_wrap() {
        assert_eq!(wrap_words("aaa bbb ccc", 8), vec!["aaa bbb", "ccc"]);
        assert_eq!(wrap_words("a\nb", 8), vec!["a", "b"]);
    }

    #[test]
    fn every_mode_is_explained() {
        for (id, _) in MODES {
            assert!(!mode_detail(id).is_empty());
        }
    }
}
