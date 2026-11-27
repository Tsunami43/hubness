//! Settings (the web app's Settings pages): Agent (default permission mode, session and
//! project instructions), Git access (HTTPS logins per host), Account (the hub's one account), and what is specific to the
//! TUI — the connection to the hub and the keyboard reference (the web app's appearance and
//! language settings belong to the browser).

use crossterm::event::{KeyCode, KeyEvent};
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Clear, List, ListItem, Paragraph, Wrap};
use serde_json::{Value, json};

use super::kit::{Nav, Sections, row, s, since};
use super::session::{MODES, enc};
use super::tabs::Tab;
use crate::core::{Action, Ctx, Handled, Screen};
use crate::ui::term::JobTerm;
use crate::ui::{Theme, block, centered};

pub fn screen(section: Option<&str>) -> Box<dyn Screen> {
    let tabs = vec![
        Tab::new("agent", "Agent", Box::new(AgentSettings::default())),
        Tab::new("git", "Git access", Box::new(GitAccess::default())),
        Tab::new("account", "Account", Box::new(Account::default())),
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

/// Where to create a personal access token on known hostings, with the repository scopes
/// filled in where the hosting allows it (the same pages as the web app's login form).
pub fn token_page(host: &str) -> Option<String> {
    let has = |name: &str| host.split(['.', '-']).any(|p| p == name);
    if host == "github.com" || host.starts_with("github.") {
        Some(format!("https://{host}/settings/tokens/new?scopes=repo,workflow&description=hubness"))
    } else if host == "gitlab.com" || has("gitlab") {
        Some(format!("https://{host}/-/user_settings/personal_access_tokens?name=hubness&scopes=read_repository,write_repository"))
    } else if host == "codeberg.org" || has("gitea") || has("forgejo") {
        Some(format!("https://{host}/user/settings/applications"))
    } else if host == "bitbucket.org" {
        Some("https://id.atlassian.com/manage-profile/security/api-tokens".into())
    } else {
        None
    }
}

/// A GitHub host: the GitHub CLI can sign in to it with a one-time code.
fn is_github(host: &str) -> bool {
    host == "github.com" || host.starts_with("github.")
}

/// Public hostings offered when adding a login (the ones that already have a login are left out).
const POPULAR_HOSTS: [&str; 4] = ["github.com", "gitlab.com", "codeberg.org", "bitbucket.org"];

/// The one-time code GitHub's device flow shows (`ABCD-1234`) in the CLI's output.
fn device_code(text: &str) -> Option<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).find_map(|w| {
        let (a, b) = w.split_once('-')?;
        let part = |p: &str| p.len() == 4 && p.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
        (part(a) && part(b)).then(|| w.to_string())
    })
}

/// A sign-in with the GitHub CLI in progress: the hub's job and the host.
struct GhLogin {
    host: String,
    term: JobTerm,
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
    gh: Option<GhLogin>,
}

impl GitAccess {
    fn selected(&self) -> Option<Value> {
        self.nav.selected().and_then(|i| self.list.as_ref()?.get(i).cloned())
    }

    fn ask_kind(&self, ctx: &mut Ctx) {
        let Some(d) = &self.draft else { return };
        let mut items = vec![
            ("token".to_string(), "Token (recommended) — a personal access token with repository access".to_string()),
            ("password".to_string(), "Login + password — many hostings refuse passwords for git".to_string()),
        ];
        if is_github(&d.host) {
            // the code sign-in is the easy way here: the token stops being the recommended one
            items[0].1 = "Token — a personal access token with repository access".to_string();
            items.insert(0, ("gh".to_string(), "Sign in with GitHub — a one-time code, no token to paste (recommended)".to_string()));
        }
        ctx.pick("kind", format!("How to sign in to {}", d.host), items);
    }

    /// The host is known: prefill the login stored for it and ask how to sign in.
    fn set_host(&mut self, host: &str, ctx: &mut Ctx) {
        let stored = self.list.iter().flatten().find(|c| c["host"] == host.trim().to_lowercase().as_str()).cloned();
        let Some(d) = &mut self.draft else { return };
        d.host = host.trim().to_lowercase();
        if let Some(c) = stored {
            d.kind = s(&c["kind"], "token").into();
            d.username = s(&c["username"], "").into();
        }
        self.ask_kind(ctx);
    }

    /// Stop a sign-in nobody finished and forget it.
    fn close_gh(&mut self, ctx: &mut Ctx) {
        if let Some(gh) = self.gh.take() {
            if gh.term.running {
                ctx.send("gh-killed", Method::POST, format!("/jobs/{}/kill", gh.term.id), Value::Null);
            }
            ctx.close_stream("gh");
        }
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
        if let Some(gh) = &self.gh {
            let mut a = vec![Action::new("o", "Open the GitHub page"), Action::new("c", "Copy the code")];
            a.push(Action::new("w", if gh.term.running { "Cancel the sign-in" } else { "Close" }));
            return a;
        }
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
                let stored: Vec<&str> = self.list.iter().flatten().filter_map(|c| c["host"].as_str()).collect();
                let mut items: Vec<(String, String)> =
                    POPULAR_HOSTS.iter().filter(|h| !stored.contains(h)).map(|h| (h.to_string(), h.to_string())).collect();
                items.push(("other".into(), "Another host…".into()));
                ctx.pick("hostpick", "Which host? (the login is used for every repository on it)", items);
            }
            "o" => {
                let url = self.gh.as_ref().map(|gh| {
                    super::harness::last_link(&gh.term.contents()).unwrap_or_else(|| format!("https://{}/login/device", gh.host))
                });
                if let Some(url) = url {
                    ctx.open_url(url);
                }
            }
            "c" => {
                if let Some(code) = self.gh.as_ref().and_then(|gh| device_code(&gh.term.contents())) {
                    super::kit::copy(ctx, &code);
                }
            }
            "w" => self.close_gh(ctx),
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
        // the sign-in window takes the keys while it is open
        if self.gh.is_some() {
            match k.code {
                KeyCode::Esc | KeyCode::Char('w') | KeyCode::Char('q') => self.close_gh(ctx),
                KeyCode::Char('o') => self.on_action("o", ctx),
                KeyCode::Char('c') => self.on_action("c", ctx),
                _ => {}
            }
            return Handled::Yes;
        }
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

    fn on_stream(&mut self, key: &str, msg: Value, ctx: &mut Ctx) {
        if key != "gh" || msg["type"] == "__ended" {
            return;
        }
        let Some(gh) = &mut self.gh else { return };
        let was_running = gh.term.running;
        gh.term.on_stream(&msg);
        if was_running && !gh.term.running {
            ctx.close_stream("gh");
            if gh.term.exit_code == Some(0) {
                // the hub saved the token when the job ended: the new login is in the list
                ctx.info(format!("Signed in to {} with GitHub", gh.host));
                self.gh = None;
                ctx.get("list", "/git-credentials");
            } else {
                ctx.error("The GitHub sign-in did not finish — w closes the window");
            }
        }
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
            "hostpick" => match result {
                Ok(Value::String(h)) if h == "other" => {
                    ctx.prompt("host", "Host (e.g. gitlab.example.com — the login is used for every repository on it)", "")
                }
                Ok(Value::String(h)) => self.set_host(&h, ctx),
                _ => self.draft = None,
            },
            "host" => match result {
                Ok(Value::String(h)) if !h.trim().is_empty() => self.set_host(&h, ctx),
                _ => self.draft = None,
            },
            "kind" if matches!(&result, Ok(Value::String(k)) if k == "gh") => {
                if let Some(d) = self.draft.take() {
                    ctx.info(format!("Starting the GitHub sign-in for {}…", d.host));
                    ctx.send(
                        format!("gh-start:{}", d.host),
                        Method::POST,
                        format!("/git-credentials/{}/gh-login", enc(&d.host)),
                        json!({ "cols": 100, "rows": 30 }),
                    );
                }
            }
            k if k.starts_with("gh-start:") => {
                let Some(v) = ctx.ok(result) else { return };
                if v["id"].is_string() {
                    let term = JobTerm::new(&v);
                    ctx.stream("gh", term.stream_path());
                    self.gh = Some(GhLogin { host: k[9..].to_string(), term });
                }
            }
            "gh-killed" => {}
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
        self.draw_list(f, area);
        if let Some(gh) = &self.gh {
            draw_gh(f, area, gh);
        }
    }
}

/// The GitHub sign-in window: the one-time code, where to enter it and what happens.
fn draw_gh(f: &mut Frame, area: Rect, gh: &GhLogin) {
    let t = Theme::get();
    let text = gh.term.contents();
    let code = device_code(&text);
    let last: Vec<String> = text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).map(String::from).collect();
    let key =
        |k: &str, label: &str| vec![Span::styled(format!(" {k} "), t.key_style()), Span::styled(format!(" {label}   "), t.muted_style())];
    let mut lines = vec![Line::raw("")];
    if !gh.term.running && gh.term.exit_code != Some(0) {
        let code = gh.term.exit_code.map_or("?".into(), |c| c.to_string());
        lines.push(Line::styled(format!("Sign-in failed (exit {code})"), Style::default().fg(t.danger)));
        lines.push(Line::raw(""));
        for l in last.iter().rev().take(6).rev() {
            lines.push(Line::styled(l.clone(), t.muted_style()));
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(key("w", "close")));
    } else if let Some(code) = code {
        lines.push(Line::styled("Enter this code on GitHub and approve access:", t.bold()));
        lines.push(Line::raw(""));
        let spaced: String = code.chars().map(|c| format!("{c} ")).collect();
        lines.push(Line::styled(
            format!("   {}", spaced.trim_end()),
            Style::default().fg(t.accent).add_modifier(ratatui::style::Modifier::BOLD),
        ));
        lines.push(Line::raw(""));
        lines.push(Line::styled(format!("https://{}/login/device", gh.host), t.accent_style()));
        lines.push(Line::raw(""));
        let mut keys = key("o", "open the page");
        keys.extend(key("c", "copy the code"));
        keys.extend(key("w", "cancel"));
        lines.push(Line::from(keys));
        lines.push(Line::raw(""));
        lines.push(Line::styled("Waiting for your approval… the login is saved by itself.", t.muted_style()));
    } else {
        lines.push(Line::styled("Preparing the GitHub CLI…", t.bold()));
        lines.push(Line::raw(""));
        if let Some(l) = last.last() {
            lines.push(Line::styled(l.clone(), t.muted_style()));
        }
    }
    let h = (lines.len() as u16 + 3).min(area.height);
    let r = centered(area, 64.min(area.width), h);
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block(format!("Sign in to {} with GitHub", gh.host), true)), r);
}

impl GitAccess {
    fn draw_list(&mut self, f: &mut Frame, area: Rect) {
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

// ---------- Account ----------

/// The hub's one account (Settings → Account): who is signed in, a new password, log out.
#[derive(Default)]
pub struct Account {
    /// `/auth/status`: `{ required, setup, user }`.
    status: Option<Value>,
    error: Option<String>,
    /// The current password, then the new one, while a change is typed.
    current: Option<String>,
    new_password: Option<String>,
}

impl Account {
    fn reload(&self, ctx: &Ctx) {
        ctx.get("status", "/auth/status");
    }

    fn user(&self) -> Option<&Value> {
        self.status.as_ref().map(|s| &s["user"]).filter(|u| u.is_object())
    }
}

impl Screen for Account {
    fn title(&self) -> String {
        "Account".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        self.reload(ctx);
    }

    fn actions(&self) -> Vec<Action> {
        if self.user().is_none() {
            return vec![Action::new("r", "Reload")];
        }
        vec![Action::new("p", "Change the password"), Action::new("l", "Log out"), Action::new("r", "Reload")]
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        match key {
            "p" if self.user().is_some() => ctx.prompt_secret("current", "Your current password"),
            "l" if self.user().is_some() => ctx.log_out(),
            "r" => self.reload(ctx),
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        if k.code == KeyCode::Enter && self.user().is_some() {
            ctx.prompt_secret("current", "Your current password");
            return Handled::Yes;
        }
        Handled::No
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        match key {
            "status" => match result {
                Ok(v) => {
                    self.status = Some(v);
                    self.error = None;
                }
                Err(e) => self.error = Some(e.to_string()),
            },
            "current" => match result {
                Ok(Value::String(current)) if !current.is_empty() => {
                    self.current = Some(current);
                    ctx.prompt_secret("new-password", "Your new password (at least 8 characters)");
                }
                _ => self.current = None,
            },
            "new-password" => match result {
                Ok(Value::String(password)) if !password.is_empty() && self.current.is_some() => {
                    self.new_password = Some(password);
                    ctx.prompt_secret("repeat-password", "Repeat the new password");
                }
                _ => self.current = None,
            },
            "repeat-password" => {
                let (current, first) = (self.current.take(), self.new_password.take());
                if let (Ok(Value::String(again)), Some(current), Some(first)) = (result, current, first) {
                    if again != first {
                        ctx.error("the passwords don't match");
                        return;
                    }
                    ctx.send("changed", Method::PUT, "/auth/password", json!({ "currentPassword": current, "password": first }));
                }
            }
            "changed" if ctx.ok(result).is_some() => ctx.info("password changed (your other sessions ended)"),
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        let t = Theme::get();
        let width = area.width.saturating_sub(4) as usize;
        let mut lines: Vec<Line> = vec![];
        match (&self.status, self.user()) {
            (None, _) => lines.push(Line::styled(" Loading…", t.muted_style())),
            (Some(_), Some(u)) => {
                lines.push(Line::from(vec![
                    Span::styled(format!(" {} ", crate::ui::theme::icon("\u{f0004}", "@")), t.accent_style()),
                    Span::raw("Signed in as "),
                    Span::styled(s(&u["username"], "").to_string(), t.bold()),
                ]));
                lines.push(Line::styled(format!("   created {}", since(s(&u["createdAt"], ""))), t.muted_style()));
                lines.push(Line::raw(""));
                for l in
                    wrap_words("hubness is a one-person hub: this is its only account. A new password ends your other sessions.", width)
                {
                    lines.push(Line::styled(format!(" {l}"), t.muted_style()));
                }
                lines.push(Line::raw(""));
                lines.push(Line::styled(" Enter/p change the password · l log out", t.muted_style()));
            }
            (Some(_), None) => {
                for l in wrap_words(
                    "The hub has no account yet and is open to this machine only. Create it in the web app (Settings → Account) or with `hubness user add <name>` on the hub machine.",
                    width,
                ) {
                    lines.push(Line::styled(format!(" {l}"), Style::default().fg(t.warn)));
                }
            }
        }
        if let Some(e) = &self.error {
            lines.push(Line::styled(format!(" {e}"), Style::default().fg(t.danger)));
        }
        f.render_widget(Paragraph::new(lines).block(block("account", true)), area);
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
            row("Change on the hub", "hubness server config data-dir <path> · then hubness server restart", W),
            Line::raw(""),
            Line::styled("This app", t.accent_style()),
            row("hubness TUI", format!("v{}", env!("CARGO_PKG_VERSION")), W),
            row("Other hubs", "Space H switches; a adds one (hubness tui --url <address> too)", W),
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

/// The keyboard reference after the app keys of `keymap` (modes, leader, per-screen keys).
pub const KEYS: &[(&str, &[(&str, &str)])] = &[
    (
        "Modes (like Vim)",
        &[
            ("NORMAL", "browse: motions and single-letter actions"),
            ("INSERT", "typing in a box, prompt, filter or terminal — Esc back to NORMAL"),
            (":", "command palette: every action of the screen and global commands"),
            ("?", "help: every key of this screen, filterable; Enter runs an action"),
        ],
    ),
    (
        "Leader: Space (Ctrl+G while typing)",
        &[
            ("Space", "go to a session, project, group or section (also Ctrl+p)"),
            ("c p s h ,", "chats · projects · system · harness · settings"),
            ("a", "+actions: the actions of this screen"),
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
    (
        "Prompts and pickers",
        &[
            ("Enter", "Confirm"),
            ("Esc", "Cancel"),
            ("type", "Filter a picker (fuzzy)"),
            ("y / n", "Answer a confirmation (Enter never confirms a deletion)"),
        ],
    ),
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
            // clamped to the end while drawing
            KeyCode::End | KeyCode::Char('G') => self.scroll = u16::MAX,
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        let t = Theme::get();
        let mut lines = vec![];
        // the app keys straight from the key table the app handles them with
        let sections = [crate::keymap::Group::Motion, crate::keymap::Group::Panes, crate::keymap::Group::Global]
            .map(|g| (crate::keymap::group_title(g).to_string(), crate::keymap::rows(g)));
        let fixed = KEYS.iter().map(|(t, keys)| (t.to_string(), keys.iter().map(|(k, l)| (k.to_string(), l.to_string())).collect()));
        let mut all: Vec<(String, Vec<(String, String)>)> = fixed.collect();
        all.splice(1..1, sections);
        for (title, keys) in &all {
            lines.push(Line::styled(title.to_string(), t.accent_style()));
            for (k, label) in keys.iter() {
                lines.push(Line::from(vec![Span::styled(format!("  {k:<22}"), t.bold()), Span::raw(label.to_string())]));
            }
            lines.push(Line::raw(""));
        }
        let max = (lines.len() as u16).saturating_sub(area.height.saturating_sub(2));
        self.scroll = self.scroll.min(max);
        f.render_widget(
            Paragraph::new(lines).scroll((self.scroll, 0)).block(block("Keyboard — ? lists the current screen's keys", false)),
            area,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_pages() {
        assert_eq!(
            token_page("github.com").as_deref(),
            Some("https://github.com/settings/tokens/new?scopes=repo,workflow&description=hubness")
        );
        assert!(token_page("git.corp-gitlab.io").is_some_and(|u| u.contains("read_repository")));
        assert_eq!(device_code("! One-time code (ABCD-12EF) copied to clipboard").as_deref(), Some("ABCD-12EF"));
        assert_eq!(device_code("Open https://github.com/login/device"), None);
        assert_eq!(token_page("code.example.com"), None);
        assert_eq!(
            token_page("gitlab.com").as_deref(),
            Some("https://gitlab.com/-/user_settings/personal_access_tokens?name=hubness&scopes=read_repository,write_repository")
        );
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
