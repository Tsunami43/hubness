//! Harness (the web app's Harness page): a tab per agent CLI — version and updates, sign-in
//! and plan, provider accounts, limits, models and the default model, usage. Install, update,
//! sign in and sign out run as terminal jobs inside the TUI (interactive, so device codes and
//! prompts work); commands that need root are only shown and copied.

use crossterm::event::{KeyCode, KeyEvent};
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use serde_json::{Value, json};

use super::kit::{bar_with, copy, num, report, row, s, since, until};
use super::tabs::{Tab, Tabs};
use crate::core::{Action, Ctx, Handled, Screen};
use crate::ui::term::JobTerm;
use crate::ui::{Theme, block};

// ---------- the screen: one tab per agent ----------

pub struct HarnessScreen {
    initial: Option<String>,
    tabs: Option<Tabs>,
}

impl HarnessScreen {
    pub fn new(kind: Option<&str>) -> Self {
        Self { initial: kind.map(String::from), tabs: None }
    }

    /// Build the tabs from the agent list (`GET /api/agents`, in the server's order).
    fn build(&mut self, agents: &Value, ctx: &mut Ctx) {
        let list = agents["agents"].as_array().cloned().unwrap_or_default();
        if list.is_empty() {
            return;
        }
        let tabs: Vec<Tab> = list
            .iter()
            .map(|a| {
                let kind = s(&a["kind"], "").to_string();
                let id = kind.clone();
                let label = s(&a["label"], &kind).to_string();
                Tab::new(id, label, Box::new(AgentTab::new(&kind)))
            })
            .collect();
        let mut tabs = Tabs::new(tabs);
        if let Some(i) = self.initial.as_deref().and_then(|k| tabs.tabs.iter().position(|t| t.id == k)) {
            tabs.active = i;
        }
        tabs.on_show(ctx);
        self.tabs = Some(tabs);
    }
}

impl Screen for HarnessScreen {
    fn title(&self) -> String {
        match &self.tabs {
            Some(t) => format!("Harness · {}", t.tabs[t.active].label),
            None => "Harness".into(),
        }
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        match &mut self.tabs {
            Some(t) => t.on_show(ctx),
            None if ctx.shared.agents["agents"].as_array().is_some_and(|a| !a.is_empty()) => {
                let agents = ctx.shared.agents.clone();
                self.build(&agents, ctx);
            }
            None => ctx.get("agents", "/agents"),
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        match &mut self.tabs {
            Some(t) => t.draw(f, area, ctx),
            None => f.render_widget(Paragraph::new("Loading the agents…").block(block("Harness", false)), area),
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        match &mut self.tabs {
            Some(t) => t.on_key(k, ctx),
            None => Handled::No,
        }
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        let result = match &mut self.tabs {
            Some(t) => match t.on_task(key, result, ctx) {
                Some(r) => r,
                None => return,
            },
            None => result,
        };
        if key == "agents"
            && let Some(v) = ctx.ok(result)
        {
            ctx.shared.agents = v.clone();
            if self.tabs.is_none() {
                self.build(&v, ctx);
            }
        }
    }

    fn on_stream(&mut self, key: &str, msg: Value, ctx: &mut Ctx) {
        if let Some(t) = &mut self.tabs {
            t.on_stream(key, msg, ctx);
        }
    }

    fn actions(&self) -> Vec<Action> {
        self.tabs.as_ref().map(|t| t.actions()).unwrap_or_default()
    }

    fn tab_names(&self) -> Vec<String> {
        self.tabs.as_ref().map(|t| t.names()).unwrap_or_default()
    }

    fn on_click(&mut self, id: &str, ctx: &mut Ctx) {
        if let Some(t) = &mut self.tabs {
            t.on_click(id, ctx);
        }
    }

    fn on_mouse(&mut self, m: crossterm::event::MouseEvent, ctx: &mut Ctx) -> Handled {
        self.tabs.as_mut().map(|t| t.on_mouse(m, ctx)).unwrap_or(Handled::No)
    }

    fn hints(&self) -> Vec<(String, String)> {
        self.tabs.as_ref().map(|t| t.hints()).unwrap_or_default()
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        if let Some(t) = &mut self.tabs {
            t.on_action(key, ctx);
        }
    }

    fn captures_text(&self) -> bool {
        self.tabs.as_ref().is_some_and(|t| t.captures_text())
    }

    fn on_paste(&mut self, text: &str, ctx: &mut Ctx) {
        if let Some(t) = &mut self.tabs {
            t.on_paste(text, ctx);
        }
    }
}

// ---------- labels ----------

/// «Weekly · all models», «Weekly · Fable», «Session (5 h)», «Daily»… as /usage names them.
pub fn limit_label(w: &Value) -> String {
    if let Some(g) = w["group"].as_str() {
        let base = match g {
            "session" => "Session (5 h)".to_string(),
            "daily" => "Daily".into(),
            "weekly" => "Weekly".into(),
            "monthly" => "Monthly".into(),
            other => other.to_string(),
        };
        return match w["scope"].as_str() {
            Some(scope) => format!("{base} · {scope}"),
            None if g == "weekly" => format!("{base} · all models"),
            None => base,
        };
    }
    match s(&w["id"], "") {
        "five_hour" => "5-hour window".into(),
        "seven_day" => "Weekly".into(),
        "seven_day_opus" => "Weekly · Opus".into(),
        "seven_day_sonnet" => "Weekly · Sonnet".into(),
        id => id.replace('_', " "),
    }
}

/// Green below 70 %, yellow below 90 %, red above (the web app's limit bars).
fn limit_color(pct: f64) -> Color {
    let t = Theme::get();
    if pct >= 90.0 {
        t.danger
    } else if pct >= 70.0 {
        t.warn
    } else {
        t.ok
    }
}

/// The version badge: text and color.
fn badge(status: &str) -> (&'static str, Color) {
    let t = Theme::get();
    match status {
        "up-to-date" => ("✓ Up to date", t.ok),
        "update-available" => ("↑ Update", t.warn),
        "not-installed" => ("✗ Not installed", t.danger),
        _ => ("? Unknown", t.muted),
    }
}

/// What an action is called in the UI.
fn action_label(id: &str) -> &'static str {
    match id {
        "install" => "Install",
        "update" => "Update",
        "login" => "Sign in",
        "login-console" => "Sign in with an API key",
        "logout" => "Sign out",
        _ => "Run",
    }
}

/// The key of an action.
fn action_key(id: &str) -> Option<&'static str> {
    match id {
        "install" => Some("i"),
        "update" => Some("U"),
        "login" => Some("L"),
        "login-console" => Some("K"),
        "logout" => Some("o"),
        _ => None,
    }
}

/// The last http(s) link in a terminal screen (a sign-in page).
pub fn last_link(text: &str) -> Option<String> {
    let joined = text.replace('\n', " ");
    joined
        .split_whitespace()
        .filter(|w| w.starts_with("https://") || w.starts_with("http://"))
        .map(|w| w.trim_end_matches(['.', ',', ')', '"', '\'', '>']).to_string())
        .next_back()
}

// ---------- one agent ----------

struct Job {
    term: JobTerm,
    action: String,
    /// Typed input not sent yet: one request at a time keeps keystrokes in order.
    pending: String,
    sending: bool,
    /// Keys go to the terminal (INSERT); Ctrl+] leaves it for NORMAL and the screen's keys.
    focused: bool,
}

impl Job {
    fn input(&mut self, data: &str, ctx: &mut Ctx) {
        self.pending.push_str(data);
        self.flush(ctx);
    }

    fn flush(&mut self, ctx: &mut Ctx) {
        if self.sending || self.pending.is_empty() || !self.term.running {
            return;
        }
        self.sending = true;
        let data = std::mem::take(&mut self.pending);
        ctx.send("input", Method::POST, format!("/jobs/{}/input", self.term.id), json!({ "data": data }));
    }
}

pub struct AgentTab {
    kind: String,
    data: Value,
    error: Option<String>,
    refreshing: bool,
    scroll: usize,
    max_scroll: usize,
    job: Option<Job>,
    /// The area the terminal gets (to start a job with the right size).
    area: Rect,
    /// A provider account picked for an operation: (provider, account id).
    account: Option<(String, String)>,
}

impl AgentTab {
    fn new(kind: &str) -> Self {
        Self {
            kind: kind.to_string(),
            data: Value::Null,
            error: None,
            refreshing: false,
            scroll: 0,
            max_scroll: 0,
            job: None,
            area: Rect::new(0, 0, 100, 30),
            account: None,
        }
    }

    fn info(&self) -> &Value {
        &self.data["info"]
    }

    fn load(&mut self, ctx: &Ctx, refresh: bool) {
        self.refreshing = refresh;
        ctx.get("info", format!("/harness/{}{}", self.kind, if refresh { "?refresh=1" } else { "" }));
    }

    /// The action `id`; a runnable one first (install may come as a script and a root
    /// package command).
    fn action(&self, id: &str) -> Option<&Value> {
        let all = self.info()["actions"].as_array()?;
        all.iter().find(|a| a["id"] == id && a["runnable"] == true).or_else(|| all.iter().find(|a| a["id"] == id))
    }

    fn installed(&self) -> bool {
        self.info()["version"]["status"] != "not-installed"
    }

    /// The actions the card offers now (as the web app shows them).
    fn offered(&self) -> Vec<&'static str> {
        let info = self.info();
        let status = s(&info["version"]["status"], "");
        let signed_in = info["auth"]["loggedIn"] == true;
        let providers = info["accountMode"] == "providers";
        let mut out = vec![];
        if !self.installed() && self.action("install").is_some() {
            out.push("install");
        }
        if self.installed() {
            if status == "update-available" && self.action("update").is_some() {
                out.push("update");
            }
            if self.action("login").is_some() && (providers || !signed_in) {
                out.push("login");
            }
            if self.action("login-console").is_some() && !signed_in && !providers {
                out.push("login-console");
            }
            if self.action("logout").is_some() && signed_in && !providers {
                out.push("logout");
            }
        }
        out
    }

    fn run(&mut self, id: &str, ctx: &mut Ctx) {
        let Some(a) = self.action(id).cloned() else {
            ctx.error(format!("{}: not available", action_label(id)));
            return;
        };
        if a["runnable"] != true {
            ctx.message(
                "Needs root",
                format!(
                    "The hub doesn’t run root commands. Run this in a terminal on the server:\n\n$ {}\n\n(c copies a command)",
                    s(&a["command"], "")
                ),
            );
            return;
        }
        let (cols, rows) = JobTerm::size_for(self.area);
        ctx.info(format!("{} {}…", action_label(id), s(&self.info()["label"], &self.kind)));
        ctx.send(format!("job:{id}"), Method::POST, format!("/harness/{}/actions/{id}", self.kind), json!({ "cols": cols, "rows": rows }));
    }

    fn close_job(&mut self, ctx: &mut Ctx) {
        if let Some(job) = self.job.take() {
            // a sign-in nobody finishes would hang forever: closing stops it (installs keep running)
            if job.term.running && job.action.starts_with("log") {
                ctx.send("killed", Method::POST, format!("/jobs/{}/kill", job.term.id), Value::Null);
            }
            ctx.close_stream("job");
            self.load(ctx, false);
        }
    }

    fn pick_model(&self, ctx: &mut Ctx) {
        let info = self.info();
        let current = info["defaultModel"].as_str();
        let models = info["models"].as_array().cloned().unwrap_or_default();
        let mark = |on: bool| if on { "✓ " } else { "  " };
        let mut items: Vec<(String, String)> = vec![];
        match info["defaultModelEdit"].as_str() {
            Some("options") => {
                let is_current = |m: &Value| match current {
                    Some(c) => m["id"] == c || m["resolved"] == c,
                    None => m["id"] == "default",
                };
                if let Some(c) = current.filter(|_| !models.iter().any(is_current)) {
                    items.push((c.to_string(), format!("✓ {c}")));
                }
                for m in &models {
                    let desc = m["description"].as_str().map(|d| format!(" — {d}")).unwrap_or_default();
                    let resolved = m["resolved"].as_str().map(|r| format!("  ({r})")).unwrap_or_default();
                    items.push((s(&m["id"], "").to_string(), format!("{}{}{desc}{resolved}", mark(is_current(m)), s(&m["label"], ""))));
                }
            }
            Some("search") => {
                items.push(("__clear".into(), format!("{}Agent default (no model set)", mark(current.is_none()))));
                items.push(("__custom".into(), "  Type a model id…".into()));
                for m in &models {
                    let id = s(&m["id"], "");
                    items.push((id.to_string(), format!("{}{id}", mark(current == Some(id)))));
                }
            }
            _ => {
                ctx.info("this agent's default model can't be set from the hub");
                return;
            }
        }
        ctx.pick("model", "Default model — used when a session has no model set", items);
    }

    fn set_model(&self, model: Option<&str>, ctx: &mut Ctx) {
        ctx.send("saved-model", Method::PUT, format!("/harness/{}/default-model", self.kind), json!({ "model": model }));
    }

    fn lines(&self, ctx: &Ctx) -> Vec<Line<'static>> {
        let t = Theme::get();
        let info = self.info();
        let mut l: Vec<Line<'static>> = vec![];
        let v = &info["version"];
        let (btext, bcolor) = badge(s(&v["status"], ""));
        let mut head = vec![Span::styled(s(&info["label"], &self.kind).to_string(), t.bold())];
        if let Some(ver) = v["installed"].as_str() {
            head.push(Span::styled(format!("  v{ver}"), t.muted_style()));
        }
        head.push(Span::styled(format!("   {btext}"), Style::default().fg(bcolor)));
        if let Some(latest) = v["latest"].as_str() {
            head.push(Span::styled(format!("  (latest {latest})"), t.muted_style()));
        }
        l.push(Line::from(head));
        let checked = self.data["checkedAt"].as_str().map(|c| format!("   checked {}", since(c))).unwrap_or_default();
        let src = if self.installed() { format!("Installed via {}", s(&v["source"], "?")) } else { "Not installed".into() };
        l.push(Line::styled(format!("{src}{checked}{}", if self.refreshing { "   checking…" } else { "" }), t.muted_style()));
        let cmd = |a: &Value, key: Option<&str>| -> Line<'static> {
            let mut spans = vec![Span::styled("  $ ", t.muted_style()), Span::raw(s(&a["command"], "").to_string())];
            if a["runnable"] != true {
                spans.push(Span::styled("   [needs root — copy it into a terminal]", Style::default().fg(t.warn)));
            } else if let Some(k) = key {
                spans.push(Span::styled(format!("   {k} run"), t.accent_style()));
            }
            Line::from(spans)
        };

        if !self.installed() {
            let installs: Vec<&Value> = info["actions"].as_array().into_iter().flatten().filter(|a| a["id"] == "install").collect();
            if !installs.is_empty() {
                l.push(Line::raw(""));
                l.push(Line::raw("Not installed on this server. Install it here or copy the command into a terminal."));
                for a in installs {
                    l.push(cmd(a, Some("i")));
                }
            }
            return l;
        }
        if v["status"] == "update-available"
            && let Some(a) = self.action("update")
        {
            l.push(Line::raw(""));
            l.push(Line::styled(format!("Update available: {}", s(&v["latest"], "")), Style::default().fg(t.warn)));
            l.push(cmd(a, Some("U")));
        }
        if info["pinned"] == true {
            l.push(Line::styled("Adapter version is pinned", t.muted_style()));
        }

        // stats
        let a = &info["auth"];
        let signed_in = a["loggedIn"] == true;
        let limits = info["limits"].as_array();
        let as_of = info["limitsAsOf"].as_str().map(|x| format!("as of {}", since(x)));
        l.push(Line::raw(""));
        l.push(Line::styled("Overview", t.accent_style()));
        const W: usize = 18;
        if signed_in && a.get("plan").is_some() {
            let detail = a["planDetail"].as_str().map(|d| format!("  ({d})")).unwrap_or_default();
            l.push(row("Plan", format!("{}{detail}", s(&a["plan"], "—")), W));
        }
        if !a.is_null() {
            let value = if signed_in { s(&a["method"], "on").to_string() } else { "No".into() };
            let until_ =
                a["expiresAt"].as_str().filter(|_| signed_in).map(|e| format!("  until {}", &e[..e.len().min(10)])).unwrap_or_default();
            l.push(row("Signed in", format!("{value}{until_}"), W));
        }
        if info["accountMode"] == "providers" {
            let accounts = info["accounts"].as_array().cloned().unwrap_or_default();
            let mut provs: Vec<&str> = accounts.iter().filter_map(|x| x["provider"].as_str()).collect();
            provs.sort();
            provs.dedup();
            let names: Vec<&str> = accounts.iter().filter_map(|x| x["providerName"].as_str()).collect();
            l.push(row("Providers", format!("{}  {}", provs.len(), names.join(", ")), W));
        }
        let models = info["models"].as_array().cloned().unwrap_or_default();
        let def = info["defaultModel"].as_str().map(|m| format!("  default: {}", m.rsplit('/').next().unwrap_or(m))).unwrap_or_default();
        let models_value =
            if models.is_empty() { "—  Listed after the first session".to_string() } else { format!("{}{def}", models.len()) };
        l.push(row("Models", models_value, W));
        if let Some(u) = info["usage"].as_object() {
            let tokens = u.get("tokens").and_then(|x| x.as_str()).map(|x| format!("{x} tokens")).unwrap_or("—".into());
            let n = |k: &str| u.get(k).map(num).unwrap_or("—".into());
            l.push(row("Usage this year", format!("{tokens} · {} sessions · {} prompts", n("sessions"), n("prompts")), W));
        }
        let pinned = if info["pinned"] == true { "  (adapter version is pinned)" } else { "" };
        l.push(row("Protocol", format!("{}{pinned}", s(&info["protocol"], "").to_uppercase()), W));

        // account(s)
        let accounts = info["accounts"].as_array().cloned().unwrap_or_default();
        if info["accountMode"] == "providers" {
            l.push(Line::raw(""));
            l.push(Line::from(vec![
                Span::styled("Providers", t.accent_style()),
                Span::styled("  model provider accounts the agent can use — p switch / sign out · L connect", t.muted_style()),
            ]));
            if accounts.is_empty() {
                l.push(Line::styled("  No providers connected yet", t.muted_style()));
            }
            for acc in &accounts {
                let many = accounts.iter().filter(|x| x["provider"] == acc["provider"]).count() > 1;
                let mut spans = vec![Span::raw(format!("  {}", s(&acc["providerName"], s(&acc["label"], ""))))];
                if many && acc["active"] == true && acc["loggedIn"] == true {
                    spans.push(Span::styled("  active", Style::default().fg(t.ok)));
                }
                if acc["loggedIn"] != true {
                    spans.push(Span::styled("  signed out", Style::default().fg(t.warn)));
                }
                let detail =
                    if many { format!("{} · {}", s(&acc["label"], ""), s(&acc["detail"], "")) } else { s(&acc["detail"], "").to_string() };
                if !detail.is_empty() {
                    spans.push(Span::styled(format!("  {detail}"), t.muted_style()));
                }
                l.push(Line::from(spans));
            }
        } else if signed_in && a.get("email").is_some() {
            l.push(Line::raw(""));
            l.push(Line::styled("Account", t.accent_style()));
            l.push(row("Email", s(&a["email"], "—").to_string(), W));
            if let Some(org) = a["org"].as_str() {
                l.push(row("Organization", org.to_string(), W));
            }
            if let Some(x) = a["extraUsage"].as_bool() {
                l.push(row("Extra usage", if x { "on" } else { "off" }, W));
            }
            if self.action("logout").is_some() {
                l.push(Line::styled("  o sign out", t.accent_style()));
            }
        } else if !accounts.is_empty() {
            l.push(Line::raw(""));
            l.push(Line::styled("Account", t.accent_style()));
            for acc in &accounts {
                let mut spans = vec![Span::raw(format!("  {}", s(&acc["label"], "")))];
                if acc["loggedIn"] != true {
                    spans.push(Span::styled("  signed out", Style::default().fg(t.warn)));
                }
                if let Some(d) = acc["detail"].as_str() {
                    spans.push(Span::styled(format!("  {d}"), t.muted_style()));
                }
                l.push(Line::from(spans));
            }
            if !signed_in && let Some(login) = self.action("login") {
                l.push(cmd(login, Some("l")));
                if self.action("login-console").is_some() {
                    l.push(Line::styled("  K sign in with an API key (pay per use) instead", t.accent_style()));
                }
            }
            if signed_in {
                l.push(Line::styled("  To switch accounts, sign out (o) and sign in with another one.", t.muted_style()));
            }
        }

        // limits
        if let Some(limits) = limits.filter(|_| signed_in) {
            l.push(Line::raw(""));
            l.push(Line::from(vec![
                Span::styled("Usage limits", t.accent_style()),
                Span::styled(format!("  {}", as_of.unwrap_or_default()), t.muted_style()),
            ]));
            if limits.is_empty() {
                l.push(Line::styled("  No data yet — appears after the first run", t.muted_style()));
            }
            let now = super::kit::now_ms();
            for w in limits {
                let pct = w["utilization"].as_f64().unwrap_or(0.0);
                let mut spans = vec![Span::raw(format!("  {:<28} ", limit_label(w)))];
                spans.extend(bar_with(pct, 30, limit_color(pct)));
                spans.push(Span::styled(format!(" {:>3}%", num(&w["utilization"])), t.bold()));
                if let Some(r) = w["resetsAt"].as_str() {
                    spans.push(Span::styled(format!("   resets {}", until(r, now)), t.muted_style()));
                }
                l.push(Line::from(spans));
            }
        }

        // default model / models
        let edit = info["defaultModelEdit"].as_str();
        if edit.is_some() && (edit == Some("search") || !models.is_empty()) {
            l.push(Line::raw(""));
            let hint = if self.kind == "claude-code" {
                "written to ~/.claude/settings.json, like Enter in /model"
            } else {
                "used when a session has no model set"
            };
            l.push(Line::from(vec![
                Span::styled("Default model", t.accent_style()),
                Span::styled(format!("  {hint} — m change"), t.muted_style()),
            ]));
            let current = info["defaultModel"].as_str();
            if edit == Some("options") {
                let is_current = |m: &Value| match current {
                    Some(c) => m["id"] == c || m["resolved"] == c,
                    None => m["id"] == "default",
                };
                if let Some(c) = current.filter(|_| !models.iter().any(is_current)) {
                    l.push(Line::styled(format!("  ✓ {c}"), t.accent_style()));
                }
                for m in &models {
                    let on = is_current(m);
                    let mut spans = vec![Span::styled(
                        format!("  {} {}", if on { "✓" } else { " " }, s(&m["label"], "")),
                        if on { t.accent_style() } else { Style::default() },
                    )];
                    if let Some(d) = m["description"].as_str() {
                        spans.push(Span::styled(format!("  {d}"), t.muted_style()));
                    }
                    if let Some(r) = m["resolved"].as_str() {
                        spans.push(Span::styled(format!("  ({r})"), t.muted_style()));
                    }
                    l.push(Line::from(spans));
                }
            } else {
                l.push(Line::raw(format!("  {}", current.unwrap_or("— (the agent's own default)"))));
                l.push(Line::styled(format!("  {} models to pick from", models.len()), t.muted_style()));
            }
        } else if !models.is_empty() {
            l.push(Line::raw(""));
            l.push(Line::styled("Available models", t.accent_style()));
            let names: Vec<String> = models.iter().map(|m| s(&m["label"], s(&m["id"], "")).to_string()).collect();
            l.push(Line::raw(format!("  {}", names.join(" · "))));
        }

        let _ = ctx;
        l
    }
}

impl Screen for AgentTab {
    fn title(&self) -> String {
        self.kind.clone()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        if self.job.is_none() {
            self.load(ctx, false);
        }
    }

    fn captures_text(&self) -> bool {
        self.job.as_ref().is_some_and(|j| j.term.running && j.focused)
    }

    fn actions(&self) -> Vec<Action> {
        if let Some(job) = &self.job {
            let mut a = vec![Action::new("w", "Close the terminal")];
            if job.term.running {
                if !job.focused {
                    a.push(Action::new("i", "Type into the terminal"));
                }
                a.push(Action::new("x", "Stop the job"));
            }
            a.push(Action::new("O", "Open the link shown in the terminal"));
            return a;
        }
        let mut a = vec![Action::new("r", "Refresh (re-check version, update, sign-in)")];
        if self.data.is_null() {
            return a;
        }
        for id in self.offered() {
            if let Some(k) = action_key(id) {
                let label = if id == "login" && self.info()["accountMode"] == "providers" { "Connect provider" } else { action_label(id) };
                a.push(Action::new(k, label));
            }
        }
        if self.info()["accountMode"] == "providers" && self.info()["accounts"].as_array().is_some_and(|x| !x.is_empty()) {
            a.push(Action::new("p", "Provider account: use / sign out"));
        }
        let edit = self.info()["defaultModelEdit"].as_str();
        if edit == Some("search") || (edit == Some("options") && self.info()["models"].as_array().is_some_and(|m| !m.is_empty())) {
            a.push(Action::new("m", "Default model"));
        }
        if self.info()["actions"].as_array().is_some_and(|x| !x.is_empty()) {
            a.push(Action::new("c", "Copy a command"));
        }
        a
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        if self.job.is_some() {
            match key {
                "w" => self.close_job(ctx),
                "x" => {
                    if let Some(j) = &self.job {
                        ctx.send("killed", Method::POST, format!("/jobs/{}/kill", j.term.id), Value::Null);
                    }
                }
                "O" => match self.job.as_ref().and_then(|j| last_link(&j.term.contents())) {
                    Some(url) => ctx.open_url(url),
                    None => ctx.info("no link in the terminal yet"),
                },
                _ => {}
            }
            return;
        }
        match key {
            "r" => self.load(ctx, true),
            "o" => ctx.confirm("logout", "Sign out", format!("Sign out of {}?", s(&self.info()["label"], &self.kind)), true),
            "p" => {
                let accounts = self.info()["accounts"].as_array().cloned().unwrap_or_default();
                let items = accounts
                    .iter()
                    .map(|a| {
                        let state = if a["active"] == true { " (active)" } else { "" };
                        (
                            format!("{}\u{1f}{}", s(&a["provider"], ""), s(&a["id"], "")),
                            format!("{} — {}{state}", s(&a["providerName"], s(&a["provider"], "")), s(&a["label"], "")),
                        )
                    })
                    .collect();
                ctx.pick("account", "Provider account", items);
            }
            "m" => self.pick_model(ctx),
            "c" => {
                let items = self.info()["actions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|a| {
                        let root = if a["runnable"] == true { "" } else { "  [needs root]" };
                        (s(&a["command"], "").to_string(), format!("{}: {}{root}", action_label(s(&a["id"], "")), s(&a["command"], "")))
                    })
                    .collect();
                ctx.pick("copy", "Copy a command", items);
            }
            k => {
                if let Some(id) = self.offered().into_iter().find(|id| action_key(id) == Some(k)) {
                    self.run(id, ctx);
                }
            }
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        if let Some(job) = &mut self.job {
            if job.term.running && job.focused {
                // Ctrl+] (some terminals report it as Ctrl+5) leaves the terminal
                let leave = k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                    && matches!(k.code, KeyCode::Char(']') | KeyCode::Char('5'));
                if leave {
                    job.focused = false;
                    ctx.info("left the terminal — i / Enter goes back in, w closes, x stops");
                } else if let Some(data) = JobTerm::key_bytes(k) {
                    job.input(&data, ctx);
                }
                return Handled::Yes;
            }
            if job.term.running {
                return match k.code {
                    KeyCode::Enter | KeyCode::Char('i') => {
                        job.focused = true;
                        Handled::Yes
                    }
                    _ => Handled::No,
                };
            }
            return match k.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                    self.close_job(ctx);
                    Handled::Yes
                }
                _ => Handled::No,
            };
        }
        match k.code {
            KeyCode::Down | KeyCode::Char('j') => self.scroll = (self.scroll + 1).min(self.max_scroll),
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::PageDown => self.scroll = (self.scroll + 10).min(self.max_scroll),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Home | KeyCode::Char('g') => self.scroll = 0,
            KeyCode::End | KeyCode::Char('G') => self.scroll = self.max_scroll,
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn on_paste(&mut self, text: &str, ctx: &mut Ctx) {
        if let Some(job) = self.job.as_mut().filter(|j| j.term.running) {
            job.input(text, ctx);
        }
    }

    fn on_stream(&mut self, key: &str, msg: Value, ctx: &mut Ctx) {
        if key != "job" {
            return;
        }
        let Some(job) = &mut self.job else { return };
        if msg["type"] == "__ended" {
            return;
        }
        let was_running = job.term.running;
        job.term.on_stream(&msg);
        if was_running && !job.term.running {
            let ok = job.term.exit_code == Some(0);
            if ok {
                ctx.info(format!("{} finished — Esc closes the terminal", job.term.title));
            } else {
                ctx.error(format!(
                    "{} exited with code {} — Esc closes the terminal",
                    job.term.title,
                    job.term.exit_code.map_or("?".into(), |c| c.to_string())
                ));
            }
            // the hub drops its harness cache when the job exits: re-read it and the agent list
            ctx.get("agents", "/agents");
        }
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        match key {
            "info" => {
                self.refreshing = false;
                match result {
                    Ok(v) => {
                        self.data = v;
                        self.error = None;
                    }
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
            "agents" => {
                if let Ok(v) = result {
                    ctx.shared.agents = v;
                }
                if self.job.is_none() {
                    self.load(ctx, false);
                }
            }
            "input" => {
                ctx.ok(result);
                if let Some(job) = &mut self.job {
                    job.sending = false;
                    job.flush(ctx);
                }
            }
            "killed" => {
                ctx.ok(result);
            }
            "logout" => {
                if matches!(result, Ok(Value::Bool(true))) {
                    self.run("logout", ctx);
                }
            }
            k if k.starts_with("job:") => {
                let Some(v) = ctx.ok(result) else { return };
                if v["id"].is_string() {
                    let term = JobTerm::new(&v);
                    ctx.stream("job", term.stream_path());
                    self.job = Some(Job { term, action: k[4..].to_string(), pending: String::new(), sending: false, focused: true });
                    ctx.info("typing goes to the terminal · Ctrl+] leaves it (then w closes, x stops)");
                } else {
                    // an action that finished at once: `{ok, output}`
                    report(ctx, action_label(&k[4..]), &v);
                    self.load(ctx, false);
                }
            }
            "copy" => {
                if let Ok(Value::String(cmd)) = result {
                    copy(ctx, &cmd);
                }
            }
            "account" => {
                if let Ok(Value::String(id)) = result
                    && let Some((provider, account)) = id.split_once('\u{1f}')
                {
                    self.account = Some((provider.to_string(), account.to_string()));
                    let items =
                        vec![("switch".to_string(), "Use this account".to_string()), ("logout".to_string(), "Sign out".to_string())];
                    ctx.pick("account-op", format!("{provider} · {account}"), items);
                }
            }
            "account-op" => match (result, self.account.clone()) {
                (Ok(Value::String(op)), Some((p, a))) if op == "logout" => {
                    ctx.confirm("account-logout", "Sign out", format!("Sign out of {p} ({a})?"), true);
                }
                (Ok(Value::String(op)), Some((p, a))) => {
                    ctx.send(
                        "account-done",
                        Method::POST,
                        format!("/harness/{}/accounts/{}/{}/{op}", self.kind, super::session::enc(&p), super::session::enc(&a)),
                        Value::Null,
                    );
                }
                _ => self.account = None,
            },
            "account-logout" => {
                if matches!(result, Ok(Value::Bool(true)))
                    && let Some((p, a)) = self.account.clone()
                {
                    ctx.send(
                        "account-done",
                        Method::POST,
                        format!("/harness/{}/accounts/{}/{}/logout", self.kind, super::session::enc(&p), super::session::enc(&a)),
                        Value::Null,
                    );
                }
            }
            "account-done" => {
                self.account = None;
                match result {
                    Ok(v) => report(ctx, "account", &v),
                    Err(e) => ctx.error(e),
                }
                self.load(ctx, false);
            }
            "model" => {
                if let Ok(Value::String(id)) = result {
                    match id.as_str() {
                        "__custom" => {
                            ctx.prompt("model-id", "Model id (provider/model)", self.info()["defaultModel"].as_str().unwrap_or_default())
                        }
                        "__clear" | "default" => self.set_model(None, ctx),
                        m => self.set_model(Some(m), ctx),
                    }
                }
            }
            "model-id" => {
                if let Ok(Value::String(m)) = result {
                    let m = m.trim().to_string();
                    self.set_model((!m.is_empty()).then_some(m.as_str()), ctx);
                }
            }
            "saved-model" => {
                if ctx.ok(result).is_some() {
                    ctx.info("default model saved");
                }
                self.load(ctx, false);
            }
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = Theme::get();
        if let Some(job) = &mut self.job {
            let [hint, term] = Layout::vertical([Constraint::Length(1), Constraint::Min(5)]).areas(area);
            self.area = term;
            let state = match (job.term.running, job.term.exit_code) {
                (true, _) => "running — keys go to the terminal".to_string(),
                (false, Some(0)) => "done — Esc closes".to_string(),
                (false, Some(c)) => format!("failed (exit code {c}) — Esc closes"),
                (false, None) => "stopped — Esc closes".to_string(),
            };
            f.render_widget(
                Paragraph::new(Line::styled(
                    " Open the sign-in link (Ctrl+] then O), finish there, then paste the code here if asked.   Ctrl+] then w close · x stop",
                    t.muted_style(),
                )),
                hint,
            );
            let title = format!("{} — {state}", job.term.title);
            job.term.draw(f, term, block(title, job.term.running));
            return;
        }
        self.area = area;
        if self.data.is_null() {
            let text = self.error.clone().unwrap_or_else(|| "Checking the harness…".into());
            f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }).block(block(self.kind.clone(), false)), area);
            return;
        }
        let mut lines = self.lines(ctx);
        if let Some(e) = &self.error {
            lines.insert(0, Line::styled(e.clone(), Style::default().fg(t.danger)));
        }
        let b = block(s(&self.info()["label"], &self.kind).to_string(), true);
        let inner = b.inner(area);
        // wrap ourselves, so scrolling knows the real height
        let wrapped: Vec<Line> = lines.into_iter().flat_map(|l| crate::ui::wrap(l, inner.width as usize)).collect();
        self.max_scroll = wrapped.len().saturating_sub(inner.height as usize);
        self.scroll = self.scroll.min(self.max_scroll);
        f.render_widget(Paragraph::new(wrapped).scroll((self.scroll as u16, 0)).block(b), area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_labels() {
        assert_eq!(limit_label(&json!({ "id": "weekly_all", "group": "weekly" })), "Weekly · all models");
        assert_eq!(limit_label(&json!({ "id": "w", "group": "weekly", "scope": "Fable" })), "Weekly · Fable");
        assert_eq!(limit_label(&json!({ "id": "session", "group": "session" })), "Session (5 h)");
        assert_eq!(limit_label(&json!({ "id": "seven_day_opus" })), "Weekly · Opus");
        assert_eq!(limit_label(&json!({ "id": "some_window" })), "some window");
    }

    #[test]
    fn links_in_terminal_text() {
        let text = "Open this page:\nhttps://example.com/device?code=AB-CD.\nthen paste the code";
        assert_eq!(last_link(text).as_deref(), Some("https://example.com/device?code=AB-CD"));
        assert_eq!(last_link("no link here"), None);
    }

    #[test]
    fn offered_actions_follow_the_state() {
        let mut tab = AgentTab::new("x");
        tab.data = json!({ "info": {
            "version": { "status": "not-installed" },
            "actions": [{ "id": "install", "command": "npm i -g x", "runnable": true }],
            "auth": null, "accountMode": "single"
        }});
        assert_eq!(tab.offered(), vec!["install"]);
        tab.data = json!({ "info": {
            "version": { "status": "update-available", "installed": "1", "latest": "2" },
            "actions": [{ "id": "update", "runnable": true }, { "id": "login", "runnable": true }, { "id": "logout", "runnable": true }],
            "auth": { "loggedIn": true }, "accountMode": "single"
        }});
        assert_eq!(tab.offered(), vec!["update", "logout"]);
    }
}
