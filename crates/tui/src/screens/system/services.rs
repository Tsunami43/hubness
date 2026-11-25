//! System → Services: systemd units of the system and user scopes (or launchd agents on
//! macOS): filter, logs, start / stop / restart of user units, ask about a unit.

use crossterm::event::{KeyCode, KeyEvent};
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{List, ListItem, Paragraph, Wrap};
use serde_json::{Value, json};

use super::{Ask, enc, unavailable};
use crate::core::{Action, Ctx, Handled, Screen};
use crate::screens::kit::{Nav, TextView, matches, report, s};
use crate::ui::{Theme, block};

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Failed,
    Active,
    All,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    All,
    System,
    User,
}

pub struct Services {
    /// `GET /system/platform`; `services: null` — no supported service manager.
    platform: Value,
    units: Option<Vec<Value>>,
    error: Option<String>,
    /// The hub answered 501: no service manager.
    missing: bool,
    filter: String,
    state: State,
    scope: Scope,
    /// The failed filter is picked once, on the first load with failed units.
    picked: bool,
    nav: Nav,
    logs: Option<(Value, TextView)>,
    ask: Ask,
}

impl Default for Services {
    fn default() -> Self {
        Self {
            platform: Value::Null,
            units: None,
            error: None,
            missing: false,
            filter: String::new(),
            state: State::Active,
            scope: Scope::All,
            picked: false,
            nav: Nav::default(),
            logs: None,
            ask: Ask::default(),
        }
    }
}

impl Services {
    fn launchd(&self) -> bool {
        self.platform["services"] == "launchd"
    }

    fn list(&self) -> Vec<&Value> {
        self.units
            .iter()
            .flatten()
            .filter(|u| match self.state {
                State::All => true,
                State::Failed => u["active"] == "failed",
                State::Active => u["active"] == "active",
            })
            .filter(|u| match self.scope {
                Scope::All => true,
                Scope::System => u["scope"] == "system",
                Scope::User => u["scope"] == "user",
            })
            .filter(|u| matches(&format!("{} {}", s(&u["name"], ""), s(&u["description"], "")), &self.filter))
            .collect()
    }

    fn selected(&self) -> Option<Value> {
        self.nav.selected().and_then(|i| self.list().get(i).map(|c| (*c).clone()))
    }

    fn path(u: &Value, rest: &str) -> String {
        format!("/system/units/{}/{}{rest}", s(&u["scope"], "system"), enc(s(&u["name"], "")))
    }

    fn open_logs(&mut self, u: Value, ctx: &mut Ctx) {
        ctx.get("logs", Self::path(&u, "/logs"));
        let title = format!("Logs · {}", s(&u["name"], ""));
        self.logs = Some((u, TextView::new(title, true)));
    }

    fn ask_about(&mut self, u: &Value, ctx: &mut Ctx) {
        let (name, scope) = (s(&u["name"], ""), s(&u["scope"], "system"));
        self.ask.start(ctx, json!({ "kind": "unit", "id": name, "scope": scope }), &format!("the {scope} unit {name}"));
    }

    fn act(&self, u: &Value, action: &str, ctx: &mut Ctx) {
        let name = s(&u["name"], "");
        ctx.info(format!("{action} {name}…"));
        ctx.send(format!("done:{action} {name}"), Method::POST, Self::path(u, &format!("/{action}")), Value::Null);
    }
}

impl Screen for Services {
    fn title(&self) -> String {
        "Services".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        if self.platform.is_null() {
            ctx.get("platform", "/system/platform");
        }
        ctx.get("units", "/system/units");
    }

    fn actions(&self) -> Vec<Action> {
        if let Some((_, v)) = &self.logs {
            let mut a = vec![Action::new("r", "Reload the logs")];
            if v.text.is_some() {
                a.push(Action::new("a", "Ask about these logs"));
            }
            return a;
        }
        if self.missing || self.platform["services"].is_null() && !self.platform.is_null() {
            return vec![];
        }
        let mut a = vec![Action::new("/", "Filter units"), Action::new("f", "Show: failed / active / all"), Action::new("r", "Refresh")];
        if !self.launchd() {
            a.push(Action::new("u", "Scope: all / system / user"));
        }
        if let Some(u) = self.selected() {
            a.push(Action::new("L", "Logs"));
            if u["scope"] == "user" {
                a.push(Action::new("R", "Restart"));
                if u["active"] == "active" {
                    a.push(Action::new("S", "Stop"));
                } else {
                    a.push(Action::new("s", "Start"));
                }
            }
            a.push(Action::new("a", "Ask about this unit"));
        }
        a
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        if let Some((u, _)) = &self.logs {
            let u = u.clone();
            match key {
                "r" => self.open_logs(u, ctx),
                "a" => self.ask_about(&u, ctx),
                _ => {}
            }
            return;
        }
        let sel = self.selected();
        match (key, sel) {
            ("/", _) => ctx.prompt("filter", "Filter units", self.filter.clone()),
            ("f", _) => {
                self.state = match self.state {
                    State::Failed => State::Active,
                    State::Active => State::All,
                    State::All => State::Failed,
                };
                self.nav.reset();
            }
            ("u", _) => {
                self.scope = match self.scope {
                    Scope::All => Scope::System,
                    Scope::System => Scope::User,
                    Scope::User => Scope::All,
                };
                self.nav.reset();
            }
            ("r", _) => ctx.get("units", "/system/units"),
            ("L", Some(u)) => self.open_logs(u, ctx),
            ("R", Some(u)) if u["scope"] == "user" => self.act(&u, "restart", ctx),
            ("s", Some(u)) if u["scope"] == "user" => self.act(&u, "start", ctx),
            ("S", Some(u)) if u["scope"] == "user" => {
                let name = s(&u["name"], "").to_string();
                ctx.confirm(&format!("stop:{name}"), "Stop", format!("Stop {name}?"), true);
            }
            ("a", Some(u)) => self.ask_about(&u, ctx),
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        if let Some((_, v)) = &mut self.logs {
            return match v.key(&k) {
                Some(false) => {
                    self.logs = None;
                    Handled::Yes
                }
                Some(true) => Handled::Yes,
                None => Handled::No,
            };
        }
        if k.code == KeyCode::Enter {
            if let Some(u) = self.selected() {
                self.open_logs(u, ctx);
            }
            return Handled::Yes;
        }
        let n = self.list().len();
        if self.nav.key(&k, n) { Handled::Yes } else { Handled::No }
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        if self.ask.on_task(key, &result, ctx) {
            return;
        }
        match key {
            "platform" => {
                if let Ok(v) = result {
                    self.platform = v;
                }
            }
            "units" => match result {
                Ok(v) => {
                    let units = v.as_array().cloned().unwrap_or_default();
                    if !self.picked && units.iter().any(|u| u["active"] == "failed") {
                        self.state = State::Failed;
                    }
                    self.picked = true;
                    self.units = Some(units);
                    self.error = None;
                    self.missing = false;
                }
                Err(e) if unavailable(&e) => self.missing = true,
                Err(e) => self.error = Some(e.to_string()),
            },
            "logs" => {
                if let Some((_, v)) = &mut self.logs {
                    match result {
                        Ok(r) => v.set(r["logs"].as_str().unwrap_or_default().to_string()),
                        Err(e) => v.set(e.to_string()),
                    }
                }
            }
            "filter" => {
                if let Ok(Value::String(f)) = result {
                    self.filter = f;
                    self.nav.reset();
                }
            }
            k if k.starts_with("stop:") => {
                if matches!(result, Ok(Value::Bool(true)))
                    && let Some(u) = self.units.iter().flatten().find(|u| u["scope"] == "user" && u["name"] == k[5..]).cloned()
                {
                    self.act(&u, "stop", ctx);
                }
            }
            k if k.starts_with("done:") => {
                match result {
                    Ok(v) => report(ctx, &k[5..], &v),
                    Err(e) => ctx.error(format!("{}: {e}", &k[5..])),
                }
                ctx.get("units", "/system/units");
            }
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        let t = Theme::get();
        if let Some((_, v)) = &mut self.logs {
            v.draw(f, area, "Esc close · a ask · r reload");
            return;
        }
        if self.missing || (!self.platform.is_null() && self.platform["services"].is_null()) {
            f.render_widget(
                Paragraph::new("No supported service manager on this server (systemd on Linux, launchd on macOS).")
                    .wrap(Wrap { trim: false })
                    .block(block("Services", false)),
                area,
            );
            return;
        }
        let launchd = self.launchd();
        let hint_h = if area.width < 120 { 2 } else { 1 };
        let [hint_area, head, body] = Layout::vertical([Constraint::Length(hint_h), Constraint::Length(1), Constraint::Min(3)]).areas(area);
        let hint = if launchd {
            "launchd agents of the hub user: restart them here; system daemons need root — ask the agent for the command."
        } else {
            "systemd units: start / stop / restart user units here; system units need root — ask the agent for the command."
        };
        f.render_widget(Paragraph::new(Line::styled(format!(" {hint}"), t.muted_style())).wrap(Wrap { trim: true }), hint_area);
        let mut spans = vec![];
        let status = match (&self.error, &self.units) {
            (Some(e), _) => e.clone(),
            (None, None) => "Loading…".into(),
            (None, Some(u)) => format!("{} units, {} failed", u.len(), u.iter().filter(|u| u["active"] == "failed").count()),
        };
        spans.push(Span::styled(format!(" {status}   "), t.muted_style()));
        for (label, on) in
            [("failed", self.state == State::Failed), ("active", self.state == State::Active), ("all", self.state == State::All)]
        {
            spans.push(Span::styled(format!(" {label} "), if on { t.key_style() } else { t.muted_style() }));
        }
        if !launchd {
            spans.push(Span::raw("   "));
            for (label, on) in
                [("all", self.scope == Scope::All), ("system", self.scope == Scope::System), ("user", self.scope == Scope::User)]
            {
                spans.push(Span::styled(format!(" {label} "), if on { t.key_style() } else { t.muted_style() }));
            }
        }
        if !self.filter.is_empty() {
            spans.push(Span::styled(format!("   filter: {}", self.filter), t.accent_style()));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), head);

        let items: Vec<ListItem> = self
            .list()
            .into_iter()
            .map(|u| {
                let color = match s(&u["active"], "") {
                    "failed" => t.danger,
                    "active" => t.ok,
                    "activating" | "reloading" => t.warn,
                    _ => t.muted,
                };
                let mut head = vec![Span::styled(" ● ", Style::default().fg(color)), Span::styled(s(&u["name"], "").to_string(), t.bold())];
                if !launchd {
                    head.push(Span::styled(format!("  [{}]", s(&u["scope"], "")), t.muted_style()));
                }
                let sub = [s(&u["description"], "").to_string(), format!("{}/{}", s(&u["active"], ""), s(&u["sub"], ""))]
                    .into_iter()
                    .filter(|x| !x.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ");
                ListItem::new(Text::from(vec![Line::from(head), Line::styled(format!("   {sub}"), t.muted_style())]))
            })
            .collect();
        let title = "units — Enter logs · f show · u scope · R restart · a ask".to_string();
        if items.is_empty() {
            let msg = if self.units.is_none() { "Loading…" } else { "Nothing here" };
            f.render_widget(Paragraph::new(Line::styled(msg, t.muted_style())).block(block(title, true)), body);
            return;
        }
        self.nav.clamp(items.len());
        f.render_stateful_widget(List::new(items).block(block(title, true)).highlight_style(t.selected()), body, &mut self.nav.state);
    }
}
