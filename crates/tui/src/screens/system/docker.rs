//! System → Docker: containers (logs, start / stop / restart, remove, ask), networks and
//! volumes (remove the unused ones).

use crossterm::event::KeyEvent;
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{List, ListItem, Paragraph, Wrap};
use serde_json::{Value, json};

use super::{Ask, enc};
use crate::core::{Action, Ctx, Handled, Screen};
use crate::screens::kit::{Nav, TextView, matches, report, s};
use crate::ui::{Theme, block};

const NO_DOCKER: &str =
    "Docker is not available\n\nThe hub could not run `docker ps`. Is Docker installed and is the user in the docker group?";

/// The filter line above a list: the hint, the text filter and the chips.
fn filter_line(hint: String, filter: &str, chips: &[(&str, bool)]) -> Line<'static> {
    let t = Theme::get();
    let mut spans = vec![Span::styled(format!(" {hint}   "), t.muted_style())];
    for (label, on) in chips {
        spans.push(Span::styled(format!(" {label} "), if *on { t.key_style() } else { t.muted_style() }));
    }
    if !filter.is_empty() {
        spans.push(Span::styled(format!("   filter: {filter}"), t.accent_style()));
    }
    Line::from(spans)
}

/// Draw a list of two-line items with the selection.
fn draw_list(f: &mut Frame, area: Rect, title: String, items: Vec<ListItem<'static>>, nav: &mut Nav, empty: &str) {
    let t = Theme::get();
    if items.is_empty() {
        f.render_widget(Paragraph::new(Line::styled(empty.to_string(), t.muted_style())).block(block(title, true)), area);
        return;
    }
    nav.clamp(items.len());
    f.render_stateful_widget(List::new(items).block(block(title, true)).highlight_style(t.selected()), area, &mut nav.state);
}

fn item(dot: (char, ratatui::style::Color), head: Vec<Span<'static>>, sub: String) -> ListItem<'static> {
    let t = Theme::get();
    let mut first = vec![Span::styled(format!(" {} ", dot.0), Style::default().fg(dot.1))];
    first.extend(head);
    ListItem::new(Text::from(vec![Line::from(first), Line::styled(format!("   {sub}"), t.muted_style())]))
}

// ---------- containers ----------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Show {
    Running,
    Stopped,
    All,
}

pub struct Containers {
    data: Value,
    error: Option<String>,
    filter: String,
    show: Show,
    nav: Nav,
    logs: Option<(String, TextView)>,
    ask: Ask,
}

impl Default for Containers {
    fn default() -> Self {
        Self {
            data: Value::Null,
            error: None,
            filter: String::new(),
            show: Show::Running,
            nav: Nav::default(),
            logs: None,
            ask: Ask::default(),
        }
    }
}

impl Containers {
    fn list(&self) -> Vec<&Value> {
        self.data["containers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| {
                let st = s(&c["state"], "");
                match self.show {
                    Show::All => true,
                    Show::Running => st == "running" || st == "restarting",
                    Show::Stopped => st != "running",
                }
            })
            .filter(|c| matches(&format!("{} {}", s(&c["name"], ""), s(&c["image"], "")), &self.filter))
            .collect()
    }

    fn selected(&self) -> Option<Value> {
        self.nav.selected().and_then(|i| self.list().get(i).map(|c| (*c).clone()))
    }

    fn load(&self, ctx: &Ctx) {
        ctx.get("list", "/system/containers");
    }

    fn open_logs(&mut self, name: &str, ctx: &mut Ctx) {
        self.logs = Some((name.to_string(), TextView::new(format!("Logs · {name}"), true)));
        ctx.get("logs", format!("/system/containers/{}/logs", enc(name)));
    }

    fn act(&self, c: &Value, action: &str, ctx: &mut Ctx) {
        let name = s(&c["name"], "");
        ctx.info(format!("{action} {name}…"));
        ctx.send(format!("done:{action} {name}"), Method::POST, format!("/system/containers/{}/{action}", enc(name)), Value::Null);
    }
}

/// Removable: stopped, and not restarting or paused (the web app's rule; the hub answers
/// 409 for a running one).
fn removable(c: &Value) -> bool {
    !matches!(s(&c["state"], ""), "running" | "restarting" | "paused")
}

impl Screen for Containers {
    fn title(&self) -> String {
        "Containers".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        self.load(ctx);
    }

    fn actions(&self) -> Vec<Action> {
        if let Some((_, v)) = &self.logs {
            let mut a = vec![Action::new("r", "Reload the logs")];
            if v.text.is_some() {
                a.push(Action::new("a", "Ask about these logs"));
            }
            return a;
        }
        if self.data["available"] == false {
            return vec![Action::new("r", "Refresh")];
        }
        let mut a =
            vec![Action::new("/", "Filter containers"), Action::new("f", "Show: running / stopped / all"), Action::new("r", "Refresh")];
        if let Some(c) = self.selected() {
            a.push(Action::new("l", "Logs"));
            if c["state"] == "running" {
                a.push(Action::new("R", "Restart"));
                a.push(Action::new("S", "Stop"));
            } else {
                a.push(Action::new("s", "Start"));
                if removable(&c) {
                    a.push(Action::new("x", "Remove"));
                }
            }
            a.push(Action::new("a", "Ask about this container"));
        }
        a
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        if let Some((name, _)) = &self.logs {
            let name = name.clone();
            match key {
                "r" => self.open_logs(&name, ctx),
                "a" => self.ask.start(ctx, json!({ "kind": "container", "id": name }), &format!("the container {name}")),
                _ => {}
            }
            return;
        }
        let sel = self.selected();
        match (key, sel) {
            ("/", _) => ctx.prompt("filter", "Filter containers", self.filter.clone()),
            ("f", _) => {
                self.show = match self.show {
                    Show::Running => Show::Stopped,
                    Show::Stopped => Show::All,
                    Show::All => Show::Running,
                };
                self.nav.reset();
            }
            ("r", _) => self.load(ctx),
            ("l", Some(c)) => self.open_logs(s(&c["name"], ""), ctx),
            ("R", Some(c)) => self.act(&c, "restart", ctx),
            ("s", Some(c)) => self.act(&c, "start", ctx),
            ("S", Some(c)) => {
                let name = s(&c["name"], "").to_string();
                ctx.confirm(&format!("stop:{name}"), "Stop", format!("Stop {name}?"), true);
            }
            ("x", Some(c)) if removable(&c) => {
                let name = s(&c["name"], "").to_string();
                ctx.confirm(
                    &format!("rm:{name}"),
                    "Remove the container",
                    format!("Remove the container {name}? This can’t be undone."),
                    true,
                );
            }
            ("a", Some(c)) => {
                let name = s(&c["name"], "").to_string();
                self.ask.start(ctx, json!({ "kind": "container", "id": name }), &format!("the container {name}"));
            }
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, _ctx: &mut Ctx) -> Handled {
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
        if k.code == crossterm::event::KeyCode::Enter {
            if let Some(c) = self.selected() {
                let name = s(&c["name"], "").to_string();
                self.open_logs(&name, _ctx);
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
            "list" => match result {
                Ok(v) => {
                    self.data = v;
                    self.error = None;
                }
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
                if matches!(result, Ok(Value::Bool(true))) {
                    let name = &k[5..];
                    ctx.info(format!("stop {name}…"));
                    ctx.send(format!("done:stop {name}"), Method::POST, format!("/system/containers/{}/stop", enc(name)), Value::Null);
                }
            }
            k if k.starts_with("rm:") => {
                if matches!(result, Ok(Value::Bool(true))) {
                    let name = &k[3..];
                    ctx.send(format!("done:remove {name}"), Method::DELETE, format!("/system/containers/{}", enc(name)), Value::Null);
                }
            }
            k if k.starts_with("done:") => {
                match result {
                    Ok(v) => report(ctx, &k[5..], &v),
                    Err(e) => ctx.error(format!("{}: {e}", &k[5..])),
                }
                self.load(ctx);
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
        if self.data["available"] == false {
            f.render_widget(Paragraph::new(NO_DOCKER).wrap(Wrap { trim: false }).block(block("Containers", false)), area);
            return;
        }
        let [head, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(3)]).areas(area);
        let all = self.data["containers"].as_array().cloned().unwrap_or_default();
        let running = all.iter().filter(|c| c["state"] == "running").count();
        let hint = match (&self.error, self.data.is_null()) {
            (Some(e), _) => e.clone(),
            (None, true) => "Loading…".into(),
            _ => format!("{running} of {} containers running", all.len()),
        };
        f.render_widget(
            Paragraph::new(filter_line(
                hint,
                &self.filter,
                &[("running", self.show == Show::Running), ("stopped", self.show == Show::Stopped), ("all", self.show == Show::All)],
            )),
            head,
        );
        let items: Vec<ListItem> = self
            .list()
            .into_iter()
            .map(|c| {
                let st = s(&c["state"], "");
                let color = match st {
                    "running" if c["health"] == "unhealthy" => t.warn,
                    "running" => t.ok,
                    "restarting" => t.danger,
                    _ => t.muted,
                };
                let mut sub = s(&c["status"], "").to_string();
                if let Some(n) = c["restartCount"].as_u64().filter(|n| *n > 0) {
                    sub.push_str(&format!(" · {n} restarts"));
                }
                if let Some(m) = c["memLimitMb"].as_f64() {
                    sub.push_str(&format!(" · mem {m} MB"));
                }
                if c["oomKilled"] == true {
                    sub.push_str(" · OOM");
                }
                if let Some(p) = c["ports"].as_str().filter(|p| !p.is_empty()) {
                    sub.push_str(&format!(" · {p}"));
                }
                item(
                    ('●', color),
                    vec![
                        Span::styled(s(&c["name"], "").to_string(), t.bold()),
                        Span::styled(format!("  {}", s(&c["image"], "")), t.muted_style()),
                    ],
                    sub,
                )
            })
            .collect();
        let title = "containers — Enter logs · s start · S stop · R restart · x remove · a ask".to_string();
        draw_list(f, body, title, items, &mut self.nav, if self.data.is_null() { "Loading…" } else { "Nothing here" });
    }
}

// ---------- networks ----------

#[derive(Default)]
pub struct Networks {
    data: Value,
    error: Option<String>,
    filter: String,
    nav: Nav,
}

impl Networks {
    fn list(&self) -> Vec<&Value> {
        self.data["networks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|n| {
                let names: Vec<&str> = n["containers"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).collect();
                matches(&format!("{} {} {}", s(&n["name"], ""), s(&n["project"], ""), names.join(" ")), &self.filter)
            })
            .collect()
    }

    fn selected(&self) -> Option<Value> {
        self.nav.selected().and_then(|i| self.list().get(i).map(|c| (*c).clone()))
    }
}

fn net_removable(n: &Value) -> bool {
    n["builtin"] != true && n["containers"].as_array().is_none_or(|c| c.is_empty())
}

impl Screen for Networks {
    fn title(&self) -> String {
        "Networks".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        ctx.get("list", "/system/networks");
    }

    fn actions(&self) -> Vec<Action> {
        let mut a = vec![Action::new("/", "Filter networks"), Action::new("r", "Refresh")];
        if self.selected().is_some_and(|n| net_removable(&n)) {
            a.push(Action::new("x", "Remove"));
        }
        a
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        match key {
            "/" => ctx.prompt("filter", "Filter networks", self.filter.clone()),
            "r" => ctx.get("list", "/system/networks"),
            "x" => {
                if let Some(n) = self.selected().filter(net_removable) {
                    let name = s(&n["name"], "").to_string();
                    ctx.confirm(&format!("rm:{name}"), "Remove the network", format!("Remove the network {name}?"), true);
                }
            }
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, _ctx: &mut Ctx) -> Handled {
        let n = self.list().len();
        if self.nav.key(&k, n) { Handled::Yes } else { Handled::No }
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        match key {
            "list" => match result {
                Ok(v) => {
                    self.data = v;
                    self.error = None;
                }
                Err(e) => self.error = Some(e.to_string()),
            },
            "filter" => {
                if let Ok(Value::String(f)) = result {
                    self.filter = f;
                    self.nav.reset();
                }
            }
            k if k.starts_with("rm:") => {
                if matches!(result, Ok(Value::Bool(true))) {
                    let name = &k[3..];
                    ctx.send(format!("done:remove {name}"), Method::DELETE, format!("/system/networks/{}", enc(name)), Value::Null);
                }
            }
            k if k.starts_with("done:") => {
                match result {
                    Ok(v) => report(ctx, &k[5..], &v),
                    Err(e) => ctx.error(format!("{}: {e}", &k[5..])),
                }
                ctx.get("list", "/system/networks");
            }
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        let t = Theme::get();
        if self.data["available"] == false {
            f.render_widget(Paragraph::new(NO_DOCKER).wrap(Wrap { trim: false }).block(block("Networks", false)), area);
            return;
        }
        let [head, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(3)]).areas(area);
        let hint = match (&self.error, self.data.is_null()) {
            (Some(e), _) => e.clone(),
            (None, true) => "Loading…".into(),
            _ => format!("{} networks", self.data["networks"].as_array().map_or(0, |a| a.len())),
        };
        f.render_widget(Paragraph::new(filter_line(hint, &self.filter, &[])), head);
        let items = self
            .list()
            .into_iter()
            .map(|n| {
                let names: Vec<&str> = n["containers"].as_array().into_iter().flatten().filter_map(|x| x.as_str()).collect();
                let mut head = vec![
                    Span::styled(s(&n["name"], "").to_string(), t.bold()),
                    Span::styled(format!("  {}", s(&n["driver"], "")), t.muted_style()),
                ];
                if n["builtin"] == true {
                    head.push(Span::styled("  [built-in]", t.muted_style()));
                }
                if n["internal"] == true {
                    head.push(Span::styled("  [internal]", t.muted_style()));
                }
                let mut sub: Vec<String> = vec![];
                if let Some(x) = n["subnet"].as_str() {
                    sub.push(x.to_string());
                }
                if let Some(p) = n["project"].as_str() {
                    sub.push(format!("compose: {p}"));
                }
                sub.push(if names.is_empty() { "nothing attached".into() } else { format!("attached: {}", names.join(", ")) });
                let color = if names.is_empty() { t.muted } else { t.ok };
                item(('●', color), head, sub.join(" · "))
            })
            .collect();
        draw_list(f, body, "networks — / filter · x remove an unused one".into(), items, &mut self.nav, "Nothing here");
    }
}

// ---------- volumes ----------

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Used {
    #[default]
    All,
    InUse,
    Unused,
}

#[derive(Default)]
pub struct Volumes {
    data: Value,
    error: Option<String>,
    filter: String,
    show: Used,
    nav: Nav,
}

impl Volumes {
    fn list(&self) -> Vec<&Value> {
        self.data["volumes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|v| {
                let links = v["links"].as_u64().unwrap_or(0);
                match self.show {
                    Used::All => true,
                    Used::InUse => links > 0,
                    Used::Unused => links == 0,
                }
            })
            .filter(|v| matches(&format!("{} {}", s(&v["name"], ""), s(&v["project"], "")), &self.filter))
            .collect()
    }

    fn selected(&self) -> Option<Value> {
        self.nav.selected().and_then(|i| self.list().get(i).map(|c| (*c).clone()))
    }
}

impl Screen for Volumes {
    fn title(&self) -> String {
        "Volumes".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        ctx.get("list", "/system/volumes");
    }

    fn actions(&self) -> Vec<Action> {
        let mut a = vec![Action::new("/", "Filter volumes"), Action::new("f", "Show: all / in use / unused"), Action::new("r", "Refresh")];
        if self.selected().is_some_and(|v| v["links"].as_u64().unwrap_or(0) == 0) {
            a.push(Action::new("x", "Remove"));
        }
        a
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        match key {
            "/" => ctx.prompt("filter", "Filter volumes", self.filter.clone()),
            "f" => {
                self.show = match self.show {
                    Used::All => Used::InUse,
                    Used::InUse => Used::Unused,
                    Used::Unused => Used::All,
                };
                self.nav.reset();
            }
            "r" => ctx.get("list", "/system/volumes"),
            "x" => {
                if let Some(v) = self.selected().filter(|v| v["links"].as_u64().unwrap_or(0) == 0) {
                    let name = s(&v["name"], "").to_string();
                    ctx.confirm(&format!("rm:{name}"), "Remove the volume", format!("Remove the volume {name}? Its data is lost."), true);
                }
            }
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, _ctx: &mut Ctx) -> Handled {
        let n = self.list().len();
        if self.nav.key(&k, n) { Handled::Yes } else { Handled::No }
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        match key {
            "list" => match result {
                Ok(v) => {
                    self.data = v;
                    self.error = None;
                }
                Err(e) => self.error = Some(e.to_string()),
            },
            "filter" => {
                if let Ok(Value::String(f)) = result {
                    self.filter = f;
                    self.nav.reset();
                }
            }
            k if k.starts_with("rm:") => {
                if matches!(result, Ok(Value::Bool(true))) {
                    let name = &k[3..];
                    let short: String = name.chars().take(24).collect();
                    ctx.send(format!("done:remove {short}"), Method::DELETE, format!("/system/volumes/{}", enc(name)), Value::Null);
                }
            }
            k if k.starts_with("done:") => {
                match result {
                    Ok(v) => report(ctx, &k[5..], &v),
                    Err(e) => ctx.error(format!("{}: {e}", &k[5..])),
                }
                ctx.get("list", "/system/volumes");
            }
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        let t = Theme::get();
        if self.data["available"] == false {
            f.render_widget(Paragraph::new(NO_DOCKER).wrap(Wrap { trim: false }).block(block("Volumes", false)), area);
            return;
        }
        let [head, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(3)]).areas(area);
        let all = self.data["volumes"].as_array().cloned().unwrap_or_default();
        let hint = match (&self.error, self.data.is_null()) {
            (Some(e), _) => e.clone(),
            (None, true) => "Reading volumes and their sizes…".into(),
            _ => format!("{} volumes, {} unused", all.len(), all.iter().filter(|v| v["links"].as_u64().unwrap_or(0) == 0).count()),
        };
        f.render_widget(
            Paragraph::new(filter_line(
                hint,
                &self.filter,
                &[("all", self.show == Used::All), ("in use", self.show == Used::InUse), ("unused", self.show == Used::Unused)],
            )),
            head,
        );
        let items = self
            .list()
            .into_iter()
            .map(|v| {
                let name = s(&v["name"], "");
                let anon = v["anonymous"] == true;
                let shown = if anon { format!("{}…", name.chars().take(12).collect::<String>()) } else { name.to_string() };
                let mut head = vec![Span::styled(shown, t.bold())];
                if anon {
                    head.push(Span::styled("  [anonymous]", t.muted_style()));
                }
                let links = v["links"].as_u64().unwrap_or(0);
                let mut sub: Vec<String> = vec![];
                if let Some(x) = v["size"].as_str() {
                    sub.push(x.to_string());
                }
                if let Some(p) = v["project"].as_str() {
                    sub.push(format!("compose: {p}"));
                }
                sub.push(if links > 0 { format!("used by {links} container(s)") } else { "unused".into() });
                item(('●', if links > 0 { t.ok } else { t.muted }), head, sub.join(" · "))
            })
            .collect();
        draw_list(f, body, "volumes — / filter · f show · x remove an unused one".into(), items, &mut self.nav, "Nothing here");
    }
}
