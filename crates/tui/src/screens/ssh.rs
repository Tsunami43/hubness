//! SSH connections: the System → SSH tab (the hub's registry: servers with keys the hub holds)
//! and the SSH tab of a project or group (which of them its agent sessions get, with a note).
//!
//! The private key never leaves the hub: a new connection gets a generated ed25519 key (or an
//! imported one) and people copy the public key into the server's `authorized_keys`. Test
//! connects once and pins the server's host keys.

use crossterm::event::{KeyCode, KeyEvent};
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Wrap};
use serde_json::{Map, Value, json};

use crate::core::{Action, Ctx, Handled, Screen};
use crate::screens::kit;
use crate::ui::{Theme, block};

/// `name [env] — user@host:port`.
/// In the Connections list: what to do with a connection whose host key is not pinned yet.
const UNTESTED_HINT: &str = "not tested — t to test";
/// In a project's / group's SSH tab: why an attached connection does nothing yet.
const UNTESTED_BOUND: &str = "not tested: the agent doesn't get it yet — t to test";

/// Did a successful "Test" pin the connection's host key? Only then do agents get it.
pub fn is_tested(c: &Value) -> bool {
    c["hostKeys"].as_str().is_some_and(|k| !k.trim().is_empty())
}

pub fn describe(c: &Value) -> String {
    format!(
        "{} — {}@{}:{}",
        c["name"].as_str().unwrap_or_default(),
        c["username"].as_str().unwrap_or_default(),
        c["host"].as_str().unwrap_or_default(),
        c["port"].as_u64().unwrap_or(22)
    )
}

/// The line that adds the public key on the server.
pub fn authorized_keys_line(public_key: &str) -> String {
    format!("mkdir -p ~/.ssh && echo '{}' >> ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys", public_key.trim())
}

/// The port typed into the form: empty is 22; anything but 1–65535 is refused (it was once
/// silently replaced with 22).
pub fn parse_port(text: &str) -> Option<u16> {
    let t = text.trim();
    if t.is_empty() {
        return Some(22);
    }
    t.parse::<u16>().ok().filter(|p| *p > 0)
}

/// Where the connection `id` is in the list.
fn position_of(items: &[Value], id: &str) -> Option<usize> {
    items.iter().position(|c| c["id"].as_str() == Some(id))
}

/// The bindings as they will be once `list` (connection id, note) is saved: shown at once, so
/// the next change builds on this one and not on an answer that is still on its way.
fn bindings_after(current: &[Value], all: &[Value], list: &[(String, String)]) -> Vec<Value> {
    list.iter()
        .filter_map(|(id, note)| {
            let connection = current.iter().map(|b| &b["connection"]).chain(all.iter()).find(|c| c["id"].as_str() == Some(id))?;
            Some(json!({ "connection": connection, "note": note }))
        })
        .collect()
}

fn move_list(list: &mut ListState, k: &KeyEvent, n: usize) -> bool {
    match k.code {
        KeyCode::Down | KeyCode::Char('j') if n > 0 => list.select(Some((list.selected().unwrap_or(0) + 1).min(n - 1))),
        KeyCode::Up | KeyCode::Char('k') => list.select(Some(list.selected().unwrap_or(0).saturating_sub(1))),
        _ => return kit::list_motion(list, k, n),
    }
    true
}

fn clamp(list: &mut ListState, n: usize) {
    if n == 0 {
        list.select(Some(0));
    } else if list.selected().is_none_or(|i| i >= n) {
        list.select(Some(n - 1));
    }
}

// ---------- System → SSH ----------

/// The fields asked one by one for a new or edited connection.
const STEPS: &[(&str, &str)] = &[
    ("name", "Name — the alias agents use: ssh <name> (e.g. prod-api)"),
    ("host", "Host — a domain name or an IP address"),
    ("username", "User on the server"),
    ("port", "Port"),
    ("description", "Description — what the server is and what agents may do there (Alt+Enter newline)"),
];

#[derive(Default)]
pub struct Connections {
    list: ListState,
    items: Vec<Value>,
    loaded: bool,
    /// The form being filled: the id when editing, the answers so far. It stays until the
    /// hub accepts it: a refused request brings it back with what was typed.
    form: Option<(Option<String>, Map<String, Value>)>,
    step: usize,
    /// The private key pasted for an import, kept with the form.
    key: Option<String>,
    /// Why the form is asked again (a refused request, a bad port), shown in the prompt.
    problem: Option<String>,
    /// The connection to select once the list is reloaded (the one just created).
    select: Option<String>,
}

impl Connections {
    fn selected(&self) -> Option<&Value> {
        self.list.selected().and_then(|i| self.items.get(i))
    }

    fn reload(&self, ctx: &mut Ctx) {
        ctx.get("list", "/ssh");
    }

    fn start(&mut self, edit: Option<Value>, ctx: &mut Ctx) {
        let id = edit.as_ref().and_then(|c| c["id"].as_str()).map(String::from);
        let mut answers = Map::new();
        if let Some(c) = edit {
            for (k, _) in STEPS {
                answers.insert((*k).into(), c[*k].clone());
            }
        }
        self.form = Some((id, answers));
        self.step = 0;
        self.key = None;
        self.problem = None;
        self.ask(ctx);
    }

    fn drop_form(&mut self) {
        self.form = None;
        self.key = None;
        self.problem = None;
    }

    fn ask(&mut self, ctx: &mut Ctx) {
        let Some((id, answers)) = &self.form else { return };
        let Some((key, title)) = STEPS.get(self.step) else {
            // an edit has no key step; a key pasted before a refused request is used again
            if id.is_some() || self.key.is_some() {
                self.submit(ctx);
            } else {
                ctx.pick(
                    "keychoice",
                    "The key",
                    vec![
                        ("generate".into(), "Generate a new key (ed25519) — recommended".into()),
                        ("import".into(), "Import an existing private key (OpenSSH, no passphrase)".into()),
                    ],
                );
            }
            return;
        };
        let current = match &answers.get(*key) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ if *key == "port" => "22".into(),
            _ => String::new(),
        };
        let what = if id.is_some() { "Edit the connection" } else { "New SSH connection" };
        let title = match self.problem.take() {
            Some(problem) => format!("{problem} · {title}"),
            None => format!("{what} · {title}"),
        };
        if *key == "description" {
            ctx.prompt_multiline("field", title, current);
        } else {
            ctx.prompt("field", title, current);
        }
    }

    /// Send the form. It is kept until the answer: an error returns to it (`on_task`).
    fn submit(&mut self, ctx: &mut Ctx) {
        let Some((id, mut body)) = self.form.clone() else { return };
        // the port step checked it; an edit keeps the number it had
        let port = match body.get("port") {
            Some(Value::String(s)) => parse_port(s).unwrap_or(22),
            Some(p) => p.as_u64().and_then(|p| u16::try_from(p).ok()).unwrap_or(22),
            None => 22,
        };
        body.insert("port".into(), json!(port));
        match id {
            Some(id) => ctx.send("saved", Method::PATCH, format!("/ssh/{}", super::session::enc(&id)), Value::Object(body)),
            None => {
                if let Some(k) = &self.key {
                    body.insert("privateKey".into(), json!(k));
                }
                ctx.info("creating the connection…");
                ctx.send("created", Method::POST, "/ssh", Value::Object(body));
            }
        }
    }
}

impl Screen for Connections {
    fn title(&self) -> String {
        "SSH".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        self.reload(ctx);
    }

    fn actions(&self) -> Vec<Action> {
        vec![
            Action::new("n", "New SSH connection (generate or import a key)"),
            Action::new("e", "Edit the selected connection"),
            Action::new("t", "Test the connection (pins the host key)"),
            Action::new("c", "Copy the public key"),
            Action::new("y", "Copy the command that adds the key on the server"),
            Action::new("x", "Delete the selected connection"),
            Action::new("r", "Reload"),
        ]
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        let sel = self.selected().cloned();
        match (key, sel) {
            ("n", _) => self.start(None, ctx),
            ("e", Some(c)) => self.start(Some(c), ctx),
            ("t", Some(c)) => {
                ctx.info(format!("connecting to {}…", c["name"].as_str().unwrap_or_default()));
                ctx.send(
                    "tested",
                    Method::POST,
                    format!("/ssh/{}/test", super::session::enc(c["id"].as_str().unwrap_or_default())),
                    json!({}),
                );
            }
            ("c", Some(c)) => kit::copy(ctx, c["publicKey"].as_str().unwrap_or_default()),
            ("y", Some(c)) => kit::copy(ctx, &authorized_keys_line(c["publicKey"].as_str().unwrap_or_default())),
            ("x", Some(c)) => {
                let name = c["name"].as_str().unwrap_or_default();
                ctx.confirm(
                    &format!("delete:{}", c["id"].as_str().unwrap_or_default()),
                    "Delete the SSH connection",
                    format!("Delete {name}? Its key is gone for good and projects lose the connection."),
                    true,
                );
            }
            ("r", _) => self.reload(ctx),
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        let n = self.items.len();
        if k.code == KeyCode::Enter {
            self.on_action(if n > 0 { "e" } else { "n" }, ctx);
        } else if !move_list(&mut self.list, &k, n) {
            return Handled::No;
        }
        Handled::Yes
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        // a cancelled prompt drops the form
        if matches!(key, "field" | "keychoice" | "privatekey") && matches!(result, Ok(Value::Null)) {
            self.drop_form();
            return;
        }
        // the hub refused the form (a name that is taken, a key it can't read): back to it,
        // with everything that was typed
        if matches!(key, "created" | "saved")
            && let Err(e) = &result
            && self.form.is_some()
        {
            let problem = e.to_string();
            if problem.contains("privateKey") {
                self.key = None;
            }
            ctx.error(&problem);
            self.problem = Some(format!("Not saved — {problem}"));
            self.step = 0;
            self.ask(ctx);
            return;
        }
        let Some(v) = ctx.ok(result) else { return };
        match key {
            "list" => {
                self.items = v.as_array().cloned().unwrap_or_default();
                self.loaded = true;
                // the connection just created: `t`, `c` and `y` act on the selected one
                if let Some(i) = self.select.take().and_then(|id| position_of(&self.items, &id)) {
                    self.list.select(Some(i));
                }
                clamp(&mut self.list, self.items.len());
            }
            "field" => {
                let Some((name, _)) = STEPS.get(self.step) else { return };
                let text = v.as_str().unwrap_or_default().trim().to_string();
                if *name == "port" && parse_port(&text).is_none() {
                    // ask again instead of quietly using 22
                    self.problem = Some(format!("“{text}” is not a port (1–65535)"));
                    self.ask(ctx);
                    return;
                }
                if let Some((_, answers)) = &mut self.form {
                    answers.insert((*name).into(), json!(text));
                }
                self.step += 1;
                self.ask(ctx);
            }
            "keychoice" => match v.as_str() {
                Some("import") => ctx.prompt_multiline("privatekey", "Paste the private key (-----BEGIN OPENSSH PRIVATE KEY----- …)", ""),
                _ => self.submit(ctx),
            },
            "privatekey" => {
                let k = v.as_str().unwrap_or_default().trim().to_string();
                if k.is_empty() {
                    self.drop_form();
                } else {
                    self.key = Some(k);
                    self.submit(ctx);
                }
            }
            "created" => {
                self.drop_form();
                self.select = v["id"].as_str().map(String::from);
                let public = v["publicKey"].as_str().unwrap_or_default();
                ctx.message(
                    format!("{} is ready", v["name"].as_str().unwrap_or_default()),
                    format!(
                        "1. Add the public key to the server:\n\n{}\n\n   (y copies this command, c the key alone)\n\n2. Press t to test. Agents get the connection once the test has passed.",
                        authorized_keys_line(public)
                    ),
                );
                self.reload(ctx);
            }
            "saved" => {
                self.drop_form();
                ctx.info("connection saved");
                self.reload(ctx);
            }
            "tested" => {
                let name = v["connection"]["name"].as_str().unwrap_or_default();
                if v["ok"] == Value::Bool(true) {
                    ctx.info(format!("{name}: connected, the host key is pinned — agents can use it now"));
                } else {
                    ctx.message(format!("{name}: can't connect"), v["output"].as_str().unwrap_or_default().to_string());
                }
                self.reload(ctx);
            }
            "deleted" => {
                ctx.info("connection deleted");
                self.reload(ctx);
            }
            k if k.starts_with("delete:") && v == Value::Bool(true) => {
                ctx.send("deleted", Method::DELETE, format!("/ssh/{}", super::session::enc(&k[7..])), Value::Null);
            }
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        let t = Theme::get();
        let [hint, list, detail] = Layout::vertical([Constraint::Length(2), Constraint::Min(3), Constraint::Length(6)]).areas(area);
        f.render_widget(
            Paragraph::new(Span::styled(
                " Servers the hub reaches with keys it holds. Attach them to a project or group (its SSH tab): its agents get them by name, keys never leave the hub.",
                t.muted_style(),
            ))
            .wrap(Wrap { trim: true }),
            hint,
        );
        let title = format!("connections ({}) — n new · e edit · t test · c / y copy key · x delete", self.items.len());
        if self.items.is_empty() {
            let msg = if self.loaded { "No SSH connections yet — n adds one." } else { "loading…" };
            f.render_widget(Paragraph::new(Span::styled(msg, t.muted_style())).block(block(title, true)), list);
        } else {
            let rows: Vec<ListItem> = self
                .items
                .iter()
                .map(|c| {
                    let pinned = is_tested(c);
                    let mut spans =
                        vec![Span::styled(" ⇄ ", t.accent_style()), Span::raw(c["name"].as_str().unwrap_or_default().to_string())];
                    spans.push(Span::styled(
                        format!(
                            "  {}@{}:{}",
                            c["username"].as_str().unwrap_or_default(),
                            c["host"].as_str().unwrap_or_default(),
                            c["port"].as_u64().unwrap_or(22)
                        ),
                        t.muted_style(),
                    ));
                    spans.push(if pinned {
                        Span::styled("  ✓ tested", Style::default().fg(t.ok))
                    } else {
                        Span::styled(format!("  {UNTESTED_HINT}"), Style::default().fg(t.warn))
                    });
                    ListItem::new(Line::from(spans))
                })
                .collect();
            f.render_stateful_widget(List::new(rows).block(block(title, true)).highlight_style(t.selected()), list, &mut self.list);
        }
        let text = match self.selected() {
            Some(c) => {
                let desc = c["description"].as_str().unwrap_or_default();
                vec![
                    Line::from(Span::raw(if desc.is_empty() { "No description — e adds one (agents read it)." } else { desc }.to_string())),
                    Line::from(Span::styled(c["publicKey"].as_str().unwrap_or_default().to_string(), t.muted_style())),
                ]
            }
            None => vec![],
        };
        f.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }).block(block("details", false)), detail);
    }
}

// ---------- a project's / group's SSH tab ----------

pub struct Bindings {
    /// `projects/<id>` or `groups/<id>`.
    base: String,
    list: ListState,
    bindings: Vec<Value>,
    all: Vec<Value>,
    loaded: bool,
    /// The connection being attached, waiting for its note.
    adding: Option<String>,
    /// Saves on their way to the hub: until the last one answers, the list shown is ours.
    saving: usize,
}

impl Bindings {
    pub fn project(id: &str) -> Self {
        Self::new(format!("projects/{}", super::session::enc(id)))
    }

    pub fn group(id: &str) -> Self {
        Self::new(format!("groups/{}", super::session::enc(id)))
    }

    fn new(base: String) -> Self {
        Self {
            base,
            list: ListState::default().with_selected(Some(0)),
            bindings: vec![],
            all: vec![],
            loaded: false,
            adding: None,
            saving: 0,
        }
    }

    fn reload(&self, ctx: &mut Ctx) {
        ctx.get("bindings", format!("/{}/ssh", self.base));
        ctx.get("all", "/ssh");
    }

    /// Save the list as `(connection id, note)` pairs. The list on screen changes at once:
    /// a second change made before the hub answers builds on the first, not on the old list.
    fn save(&mut self, list: Vec<(String, String)>, ctx: &mut Ctx) {
        self.bindings = bindings_after(&self.bindings, &self.all, &list);
        clamp(&mut self.list, self.bindings.len());
        self.saving += 1;
        let bindings: Vec<Value> = list.into_iter().map(|(id, note)| json!({ "connectionId": id, "note": note })).collect();
        ctx.send("saved", Method::PUT, format!("/{}/ssh", self.base), json!({ "bindings": bindings }));
    }

    fn current(&self) -> Vec<(String, String)> {
        self.bindings
            .iter()
            .map(|b| (b["connection"]["id"].as_str().unwrap_or_default().to_string(), b["note"].as_str().unwrap_or_default().to_string()))
            .collect()
    }

    fn selected(&self) -> Option<&Value> {
        self.list.selected().and_then(|i| self.bindings.get(i))
    }
}

impl Screen for Bindings {
    fn title(&self) -> String {
        "SSH".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        self.reload(ctx);
    }

    fn actions(&self) -> Vec<Action> {
        vec![
            Action::new("a", "Attach an SSH connection"),
            Action::new("e", "Edit the note of the selected connection"),
            Action::new("t", "Test the selected connection (agents get only tested ones)"),
            Action::new("x", "Detach the selected connection"),
            Action::new("r", "Reload"),
        ]
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        match key {
            "a" => {
                let bound = self.current();
                let items: Vec<(String, String)> = self
                    .all
                    .iter()
                    .filter(|c| !bound.iter().any(|(id, _)| c["id"].as_str() == Some(id)))
                    .map(|c| (c["id"].as_str().unwrap_or_default().to_string(), describe(c)))
                    .collect();
                if items.is_empty() {
                    let msg = if self.all.is_empty() {
                        "No SSH connections yet — add them in System → SSH."
                    } else {
                        "Every connection is attached already."
                    };
                    ctx.info(msg);
                } else {
                    ctx.pick("attach", "Attach an SSH connection", items);
                }
            }
            "e" => {
                if let Some(b) = self.selected() {
                    let id = b["connection"]["id"].as_str().unwrap_or_default().to_string();
                    let note = b["note"].as_str().unwrap_or_default().to_string();
                    ctx.prompt(
                        &format!("note:{id}"),
                        format!("What {} is for here (agents read it)", b["connection"]["name"].as_str().unwrap_or_default()),
                        note,
                    );
                }
            }
            "x" => {
                if let Some(b) = self.selected() {
                    let id = b["connection"]["id"].as_str().unwrap_or_default();
                    let list = self.current().into_iter().filter(|(c, _)| c != id).collect();
                    self.save(list, ctx);
                }
            }
            "t" => {
                if let Some(b) = self.selected() {
                    let id = b["connection"]["id"].as_str().unwrap_or_default();
                    ctx.info(format!("connecting to {}…", b["connection"]["host"].as_str().unwrap_or_default()));
                    ctx.send("tested", Method::POST, format!("/ssh/{}/test", super::session::enc(id)), json!({}));
                }
            }
            "r" => self.reload(ctx),
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        let n = self.bindings.len();
        if k.code == KeyCode::Enter {
            self.on_action(if n > 0 { "e" } else { "a" }, ctx);
        } else if !move_list(&mut self.list, &k, n) {
            return Handled::No;
        }
        Handled::Yes
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        if key == "saved" {
            self.saving = self.saving.saturating_sub(1);
            if result.is_err() && self.saving == 0 {
                // what is shown was never saved: back to what the hub has
                self.reload(ctx);
            }
        }
        let Some(v) = ctx.ok(result) else { return };
        match key {
            // an answer older than a save still on its way would bring back the old list
            "bindings" | "saved" if self.saving > 0 => {}
            "bindings" | "saved" => {
                self.bindings = v.as_array().cloned().unwrap_or_default();
                self.loaded = true;
                clamp(&mut self.list, self.bindings.len());
            }
            "all" => self.all = v.as_array().cloned().unwrap_or_default(),
            "tested" => {
                let name = v["connection"]["name"].as_str().unwrap_or_default();
                if v["ok"] == Value::Bool(true) {
                    ctx.info(format!("{name}: connected — agents here can use it now"));
                } else {
                    ctx.message(format!("{name}: can't connect"), v["output"].as_str().unwrap_or_default().to_string());
                }
                // the answer carries the connection as it is now (its pinned host key)
                apply_tested(&mut self.bindings, &v["connection"]);
                apply_tested_all(&mut self.all, &v["connection"]);
            }
            "attach" => {
                let Some(id) = v.as_str() else { return };
                self.adding = Some(id.to_string());
                let name =
                    self.all.iter().find(|c| c["id"] == id).map(|c| c["name"].as_str().unwrap_or_default().to_string()).unwrap_or_default();
                ctx.prompt("addnote", format!("What {name} is for here, e.g. \"deploy after tests pass\" (optional)"), "");
            }
            "addnote" => {
                let Some(id) = self.adding.take() else { return };
                if v.is_null() {
                    return;
                }
                let mut list = self.current();
                list.push((id, v.as_str().unwrap_or_default().trim().to_string()));
                self.save(list, ctx);
            }
            k if k.starts_with("note:") => {
                let Some(note) = v.as_str() else { return };
                let id = &k[5..];
                let list = self.current().into_iter().map(|(c, n)| if c == id { (c, note.trim().to_string()) } else { (c, n) }).collect();
                self.save(list, ctx);
            }
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        let t = Theme::get();
        let [hint, list] = Layout::vertical([Constraint::Length(2), Constraint::Min(3)]).areas(area);
        f.render_widget(
            Paragraph::new(Span::styled(
                " Agent sessions here reach these servers by name (ssh <name>); they see each one's description and your note. The keys stay in the hub.",
                t.muted_style(),
            ))
            .wrap(Wrap { trim: true }),
            hint,
        );
        let title = format!("SSH connections ({}) — a attach · e note · t test · x detach", self.bindings.len());
        if self.bindings.is_empty() {
            let msg = if self.loaded { "None attached — a attaches one (create them in System → SSH)." } else { "loading…" };
            f.render_widget(Paragraph::new(Span::styled(msg, t.muted_style())).block(block(title, true)), list);
            return;
        }
        let rows: Vec<ListItem> = self
            .bindings
            .iter()
            .map(|b| {
                let c = &b["connection"];
                let mut first = vec![Span::styled(" ⇄ ", t.accent_style()), Span::raw(c["name"].as_str().unwrap_or_default().to_string())];
                first.push(Span::styled(
                    format!("  {}@{}", c["username"].as_str().unwrap_or_default(), c["host"].as_str().unwrap_or_default()),
                    t.muted_style(),
                ));
                let note = b["note"].as_str().unwrap_or_default();
                let second = if note.is_empty() { "   no note".to_string() } else { format!("   {note}") };
                let mut lines = vec![Line::from(first), Line::from(Span::styled(second, t.muted_style()))];
                if !is_tested(c) {
                    lines.push(Line::from(Span::styled(format!("   {UNTESTED_BOUND}"), Style::default().fg(t.warn))));
                }
                ListItem::new(lines)
            })
            .collect();
        f.render_stateful_widget(List::new(rows).block(block(title, true)).highlight_style(t.selected()), list, &mut self.list);
    }
}

/// Put the connection a test answered with into the bindings that hold it.
fn apply_tested(bindings: &mut [Value], connection: &Value) {
    for b in bindings.iter_mut().filter(|b| b["connection"]["id"] == connection["id"]) {
        b["connection"] = connection.clone();
    }
}

fn apply_tested_all(all: &mut [Value], connection: &Value) {
    for c in all.iter_mut().filter(|c| c["id"] == connection["id"]) {
        *c = connection.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tested_connections_are_told_apart_and_a_test_updates_the_binding() {
        assert!(!is_tested(&json!({ "hostKeys": "" })) && !is_tested(&json!({ "hostKeys": "  " })) && !is_tested(&json!({})));
        assert!(is_tested(&json!({ "hostKeys": "h ssh-ed25519 AAAA" })));
        let mut bindings = vec![
            json!({ "connection": { "id": "a", "name": "alpha", "hostKeys": "" }, "note": "keep me" }),
            json!({ "connection": { "id": "b", "name": "beta", "hostKeys": "" }, "note": "" }),
        ];
        apply_tested(&mut bindings, &json!({ "id": "a", "name": "alpha", "hostKeys": "h ssh-ed25519 AAAA" }));
        assert!(is_tested(&bindings[0]["connection"]) && !is_tested(&bindings[1]["connection"]));
        assert_eq!(bindings[0]["note"], "keep me", "the note is the binding's, not the connection's");
        let mut all = vec![json!({ "id": "a", "hostKeys": "" }), json!({ "id": "b", "hostKeys": "" })];
        apply_tested_all(&mut all, &json!({ "id": "b", "hostKeys": "k" }));
        assert!(!is_tested(&all[0]) && is_tested(&all[1]));
    }

    #[test]
    fn ports() {
        assert_eq!(parse_port(""), Some(22));
        assert_eq!(parse_port("  "), Some(22));
        assert_eq!(parse_port(" 2222 "), Some(2222));
        assert_eq!(parse_port("65535"), Some(65535));
        for bad in ["0", "65536", "22o2", "-22", "2 2", "22.0", "ssh", "１２"] {
            assert_eq!(parse_port(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_command_for_the_server_takes_the_key_as_one_word() {
        let line = authorized_keys_line("  ssh-ed25519 AAAAC3Nza hubness:prod-api\n");
        assert!(line.contains("echo 'ssh-ed25519 AAAAC3Nza hubness:prod-api' >> ~/.ssh/authorized_keys"), "trimmed, in single quotes");
        assert!(line.starts_with("mkdir -p ~/.ssh && ") && line.ends_with("chmod 600 ~/.ssh/authorized_keys"));
        // the default port
        assert_eq!(describe(&json!({ "name": "box", "username": "root", "host": "h" })), "box — root@h:22");
    }

    fn conn(id: &str) -> Value {
        json!({ "id": id, "name": id, "username": "u", "host": "h", "port": 22, "publicKey": format!("ssh-ed25519 KEY-{id}") })
    }

    #[test]
    fn the_new_connection_is_selected_and_the_form_survives_a_refusal() {
        let mut ctx = crate::core::test_ctx();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let _guard = rt.enter();
        let mut s = Connections::default();
        s.on_task("list", Ok(json!([conn("alpha"), conn("zeta")])), &mut ctx);
        s.list.select(Some(0));

        // fill the form: a bad port is asked again, not replaced with 22
        s.on_action("n", &mut ctx);
        for answer in ["beta", "example.com", "deploy"] {
            s.on_task("field", Ok(json!(answer)), &mut ctx);
        }
        s.on_task("field", Ok(json!("22o2")), &mut ctx);
        assert_eq!(s.step, 3, "still on the port");
        assert!(
            matches!(ctx.requests.last(), Some(crate::core::Request::Modal(crate::core::Modal::Input { title, .. })) if title.contains("is not a port"))
        );
        for answer in ["2222", "API"] {
            s.on_task("field", Ok(json!(answer)), &mut ctx);
        }
        s.on_task("keychoice", Ok(json!("import")), &mut ctx);
        s.on_task("privatekey", Ok(json!("-----BEGIN OPENSSH PRIVATE KEY-----")), &mut ctx);

        // the hub refuses it: the form comes back with everything that was typed
        ctx.requests.clear();
        s.on_task("created", Err(anyhow::anyhow!("409: An SSH connection named beta exists")), &mut ctx);
        let (_, answers) = s.form.as_ref().expect("the form is kept");
        assert_eq!(
            (answers["name"].as_str(), answers["host"].as_str(), answers["port"].as_str()),
            (Some("beta"), Some("example.com"), Some("2222"))
        );
        assert!(s.key.is_some(), "and the pasted key");
        assert_eq!(s.step, 0);
        assert!(
            matches!(ctx.requests.last(), Some(crate::core::Request::Modal(crate::core::Modal::Input { title, value, .. })) if title.starts_with("Not saved — 409") && value == "beta")
        );
        // a key the hub can't read is asked for again
        s.on_task("created", Err(anyhow::anyhow!("400: privateKey: not an OpenSSH private key")), &mut ctx);
        assert!(s.key.is_none() && s.form.is_some());

        // accepted: the form is gone and the new connection is the selected one
        s.on_task("created", Ok(conn("beta")), &mut ctx);
        assert!(s.form.is_none());
        s.on_task("list", Ok(json!([conn("alpha"), conn("beta"), conn("zeta")])), &mut ctx);
        assert_eq!(s.selected().and_then(|c| c["id"].as_str()), Some("beta"), "t / c / y act on the new connection");
        // a later reload keeps the user's selection
        s.list.select(Some(2));
        s.on_task("list", Ok(json!([conn("alpha"), conn("beta"), conn("zeta")])), &mut ctx);
        assert_eq!(s.list.selected(), Some(2));
        // the list shrank under the selection
        s.on_task("list", Ok(json!([conn("alpha")])), &mut ctx);
        assert_eq!(s.list.selected(), Some(0));
        assert_eq!(position_of(&s.items, "missing"), None);

        // Esc in a prompt drops the form
        s.on_action("n", &mut ctx);
        s.on_task("field", Ok(Value::Null), &mut ctx);
        assert!(s.form.is_none());
    }

    #[test]
    fn a_second_change_builds_on_the_first_before_the_hub_answers() {
        let mut ctx = crate::core::test_ctx();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let _guard = rt.enter();
        let bound =
            |ids: &[&str]| Value::Array(ids.iter().map(|id| json!({ "connection": conn(id), "note": format!("note {id}") })).collect());
        let ids = |s: &Bindings| s.current().into_iter().map(|(id, _)| id).collect::<Vec<_>>();
        let mut s = Bindings::project("p1");
        s.on_task("all", Ok(json!([conn("a"), conn("b"), conn("c"), conn("d")])), &mut ctx);
        s.on_task("bindings", Ok(bound(&["a", "b", "c"])), &mut ctx);

        // x, j, x quickly: the second detach must not bring the first connection back
        s.list.select(Some(0));
        s.on_action("x", &mut ctx);
        assert_eq!(ids(&s), ["b", "c"], "shown at once");
        s.list.select(Some(1));
        s.on_action("x", &mut ctx);
        assert_eq!(ids(&s), ["b"]);
        assert_eq!(s.saving, 2);
        // the first answer (and a reload that started earlier) are older than the second save
        s.on_task("saved", Ok(bound(&["b", "c"])), &mut ctx);
        s.on_task("bindings", Ok(bound(&["a", "b", "c"])), &mut ctx);
        assert_eq!(ids(&s), ["b"]);
        s.on_task("saved", Ok(bound(&["b"])), &mut ctx);
        assert_eq!((ids(&s), s.saving), (vec!["b".to_string()], 0));

        // attaching takes the connection from the registry, with its note
        s.on_task("attach", Ok(json!("d")), &mut ctx);
        s.on_task("addnote", Ok(json!(" deploys ")), &mut ctx);
        assert_eq!(s.current(), [("b".to_string(), "note b".to_string()), ("d".to_string(), "deploys".to_string())]);
        assert_eq!(s.bindings[1]["connection"]["publicKey"], "ssh-ed25519 KEY-d");
        // an id that is nowhere is left out rather than shown empty
        assert_eq!(bindings_after(&s.bindings, &s.all, &[("ghost".into(), String::new())]), Vec::<Value>::new());
        // a failed save: the next answer from the hub is taken again
        s.on_task("saved", Err(anyhow::anyhow!("500: boom")), &mut ctx);
        assert_eq!(s.saving, 0);
        s.on_task("bindings", Ok(bound(&["b"])), &mut ctx);
        assert_eq!(ids(&s), ["b"]);
    }

    #[test]
    fn lines() {
        let c = json!({ "name": "prod-api", "username": "deploy", "host": "10.0.0.5", "port": 2222 });
        assert_eq!(describe(&c), "prod-api — deploy@10.0.0.5:2222");
        assert_eq!(
            authorized_keys_line("ssh-ed25519 AAAA x\n"),
            "mkdir -p ~/.ssh && echo 'ssh-ed25519 AAAA x' >> ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys"
        );
    }
}
