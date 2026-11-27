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
pub fn describe(c: &Value) -> String {
    let env = c["environment"].as_str().unwrap_or_default();
    let env = if env.is_empty() { String::new() } else { format!(" [{env}]") };
    format!(
        "{}{env} — {}@{}:{}",
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

fn env_style(t: &Theme, env: &str) -> Style {
    match env.to_ascii_lowercase().as_str() {
        "prod" | "production" | "live" => Style::default().fg(t.danger),
        "staging" | "stage" | "test" => Style::default().fg(t.warn),
        _ => t.accent_style(),
    }
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
    ("environment", "Environment — dev, staging, prod… (agents see it)"),
    ("description", "Description — what the server is and what agents may do there (Alt+Enter newline)"),
];

#[derive(Default)]
pub struct Connections {
    list: ListState,
    items: Vec<Value>,
    loaded: bool,
    /// The form being filled: the id when editing, the answers so far.
    form: Option<(Option<String>, Map<String, Value>)>,
    step: usize,
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
        self.ask(ctx);
    }

    fn ask(&mut self, ctx: &mut Ctx) {
        let Some((id, answers)) = &self.form else { return };
        let Some((key, title)) = STEPS.get(self.step) else {
            if id.is_some() {
                self.submit(None, ctx);
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
        let title = format!("{what} · {title}");
        if *key == "description" {
            ctx.prompt_multiline("field", title, current);
        } else {
            ctx.prompt("field", title, current);
        }
    }

    fn submit(&mut self, private_key: Option<String>, ctx: &mut Ctx) {
        let Some((id, mut body)) = self.form.take() else { return };
        let port = body.get("port").and_then(|p| p.as_str().map(|s| s.trim().parse::<u64>().ok()).unwrap_or(p.as_u64())).unwrap_or(22);
        body.insert("port".into(), json!(port));
        match id {
            Some(id) => ctx.send("saved", Method::PATCH, format!("/ssh/{}", super::session::enc(&id)), Value::Object(body)),
            None => {
                if let Some(k) = private_key {
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
            self.form = None;
            return;
        }
        let Some(v) = ctx.ok(result) else { return };
        match key {
            "list" => {
                self.items = v.as_array().cloned().unwrap_or_default();
                self.loaded = true;
                clamp(&mut self.list, self.items.len());
            }
            "field" => {
                let Some((name, _)) = STEPS.get(self.step) else { return };
                if let Some((_, answers)) = &mut self.form {
                    answers.insert((*name).into(), json!(v.as_str().unwrap_or_default().trim()));
                }
                self.step += 1;
                self.ask(ctx);
            }
            "keychoice" => match v.as_str() {
                Some("import") => ctx.prompt_multiline("privatekey", "Paste the private key (-----BEGIN OPENSSH PRIVATE KEY----- …)", ""),
                _ => self.submit(None, ctx),
            },
            "privatekey" => {
                let k = v.as_str().unwrap_or_default().trim().to_string();
                if k.is_empty() {
                    self.form = None;
                } else {
                    self.submit(Some(k), ctx);
                }
            }
            "created" => {
                let public = v["publicKey"].as_str().unwrap_or_default();
                ctx.message(
                    format!("{} is ready", v["name"].as_str().unwrap_or_default()),
                    format!(
                        "Add the public key to the server, then press t to test:\n\n{}\n\nc copies the key, y copies this command.",
                        authorized_keys_line(public)
                    ),
                );
                self.reload(ctx);
            }
            "saved" => {
                ctx.info("connection saved");
                self.reload(ctx);
            }
            "tested" => {
                let name = v["connection"]["name"].as_str().unwrap_or_default();
                if v["ok"] == Value::Bool(true) {
                    ctx.info(format!("{name}: connected, the host key is pinned"));
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
                    let env = c["environment"].as_str().unwrap_or_default();
                    let pinned = !c["hostKeys"].as_str().unwrap_or_default().is_empty();
                    let mut spans =
                        vec![Span::styled(" ⇄ ", t.accent_style()), Span::raw(c["name"].as_str().unwrap_or_default().to_string())];
                    if !env.is_empty() {
                        spans.push(Span::styled(format!(" [{env}]"), env_style(&t, env)));
                    }
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
                        Span::styled("  not tested", t.muted_style())
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
}

impl Bindings {
    pub fn project(id: &str) -> Self {
        Self::new(format!("projects/{}", super::session::enc(id)))
    }

    pub fn group(id: &str) -> Self {
        Self::new(format!("groups/{}", super::session::enc(id)))
    }

    fn new(base: String) -> Self {
        Self { base, list: ListState::default().with_selected(Some(0)), bindings: vec![], all: vec![], loaded: false, adding: None }
    }

    fn reload(&self, ctx: &mut Ctx) {
        ctx.get("bindings", format!("/{}/ssh", self.base));
        ctx.get("all", "/ssh");
    }

    /// Save the list as `(connection id, note)` pairs.
    fn save(&self, list: Vec<(String, String)>, ctx: &mut Ctx) {
        let bindings: Vec<Value> = list.into_iter().map(|(id, note)| json!({ "connectionId": id, "note": note })).collect();
        ctx.send("bindings", Method::PUT, format!("/{}/ssh", self.base), json!({ "bindings": bindings }));
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
        let Some(v) = ctx.ok(result) else { return };
        match key {
            "bindings" => {
                self.bindings = v.as_array().cloned().unwrap_or_default();
                self.loaded = true;
                clamp(&mut self.list, self.bindings.len());
            }
            "all" => self.all = v.as_array().cloned().unwrap_or_default(),
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
                " Agent sessions here reach these servers by name (ssh <name>); they see each one's environment, description and your note. The keys stay in the hub.",
                t.muted_style(),
            ))
            .wrap(Wrap { trim: true }),
            hint,
        );
        let title = format!("SSH connections ({}) — a attach · e note · x detach", self.bindings.len());
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
                let env = c["environment"].as_str().unwrap_or_default();
                let mut first = vec![Span::styled(" ⇄ ", t.accent_style()), Span::raw(c["name"].as_str().unwrap_or_default().to_string())];
                if !env.is_empty() {
                    first.push(Span::styled(format!(" [{env}]"), env_style(&t, env)));
                }
                first.push(Span::styled(
                    format!("  {}@{}", c["username"].as_str().unwrap_or_default(), c["host"].as_str().unwrap_or_default()),
                    t.muted_style(),
                ));
                let note = b["note"].as_str().unwrap_or_default();
                let second = if note.is_empty() { "   no note".to_string() } else { format!("   {note}") };
                ListItem::new(vec![Line::from(first), Line::from(Span::styled(second, t.muted_style()))])
            })
            .collect();
        f.render_stateful_widget(List::new(rows).block(block(title, true)).highlight_style(t.selected()), list, &mut self.list);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines() {
        let c = json!({ "name": "prod-api", "environment": "prod", "username": "deploy", "host": "10.0.0.5", "port": 2222 });
        assert_eq!(describe(&c), "prod-api [prod] — deploy@10.0.0.5:2222");
        assert_eq!(
            authorized_keys_line("ssh-ed25519 AAAA x\n"),
            "mkdir -p ~/.ssh && echo 'ssh-ed25519 AAAA x' >> ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys"
        );
    }
}
