//! The Run tab of a project (the web app's RunPanel and PreviewPanel): named run commands
//! with status and port, start / stop with the live output (an interactive terminal),
//! add / edit / delete, the primary command, suggestions from the repository, and the
//! preview proxy opened in the browser.

use crossterm::event::{KeyCode, KeyEvent};
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use serde_json::{Value, json};

use super::termpane::{LEAVE_HINT, TermPane, is_leave};
use crate::core::{Action, Ctx, Handled, Screen};
use crate::ui::{Theme, block};

/// The address of the hub's preview proxy port: the hub's host with another port.
pub fn preview_url(base_url: &str, port: u64) -> String {
    let (scheme, rest) = base_url.split_once("://").unwrap_or(("http", base_url));
    let authority = rest.split('/').next().unwrap_or_default();
    let authority = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
    let host = if authority.starts_with('[') {
        authority.split_inclusive(']').next().unwrap_or(authority)
    } else {
        authority.split(':').next().unwrap_or(authority)
    };
    let host = if host.is_empty() { "127.0.0.1" } else { host };
    format!("{scheme}://{host}:{port}/")
}

/// A port typed by the user (1–65535).
fn parse_port(s: &str) -> Option<u16> {
    s.trim().parse::<u16>().ok().filter(|p| *p > 0)
}

fn is_running(r: &Value) -> bool {
    r["job"]["status"] == "running"
}

/// The run form being filled in (one prompt after another).
#[derive(Default, Clone)]
struct Draft {
    /// The edited run, None for a new one.
    id: Option<String>,
    name: String,
    command: String,
    port: Option<u16>,
}

pub struct RunTab {
    project: String,
    runs: Vec<Value>,
    suggestions: Vec<Value>,
    list: ListState,
    term: TermPane,
    /// Keys go to the output terminal.
    typing: bool,
    draft: Draft,
    polling: bool,
    loaded: bool,
}

impl RunTab {
    pub fn new(project: &str) -> Self {
        Self {
            project: project.to_string(),
            runs: vec![],
            suggestions: vec![],
            list: ListState::default().with_selected(Some(0)),
            term: TermPane::new("out"),
            typing: false,
            draft: Draft::default(),
            polling: false,
            loaded: false,
        }
    }

    fn path(&self, rest: &str) -> String {
        format!("/projects/{}{rest}", self.project)
    }

    fn selected(&self) -> Option<&Value> {
        self.list.selected().and_then(|i| self.runs.get(i))
    }

    fn reload(&self, ctx: &mut Ctx) {
        ctx.get("runs", self.path("/runs"));
    }

    /// Show the output of the selected run (its current or last job).
    fn follow_selected(&mut self, ctx: &mut Ctx) {
        match self.selected().map(|r| r["job"].clone()) {
            Some(job) if job.is_object() => self.term.attach(&job, ctx),
            _ => {
                self.term.detach(ctx);
                self.typing = false;
            }
        }
    }

    fn start(&mut self, r: &Value, ctx: &mut Ctx) {
        let rid = r["id"].as_str().unwrap_or_default();
        ctx.send("started", Method::POST, self.path(&format!("/runs/{rid}/start")), self.term.size_body());
    }

    fn toggle(&mut self, r: Value, ctx: &mut Ctx) {
        let rid = r["id"].as_str().unwrap_or_default().to_string();
        if is_running(&r) {
            ctx.send("stopped", Method::POST, self.path(&format!("/runs/{rid}/stop")), Value::Null);
        } else {
            self.start(&r, ctx);
        }
    }

    fn select_id(&mut self, id: &Value) {
        if let Some(i) = self.runs.iter().position(|r| &r["id"] == id) {
            self.list.select(Some(i));
        }
    }

    fn preview(&mut self, r: &Value, ctx: &mut Ctx) {
        match r["previewPort"].as_u64() {
            Some(port) if is_running(r) => ctx.open_url(preview_url(&ctx.shared.base_url, port)),
            _ => ctx.info("start the command first: preview needs a running command with a port"),
        }
    }

    /// Poll the run states every 3 s while something runs (ports and statuses change).
    fn schedule_poll(&mut self, ctx: &mut Ctx) {
        if self.polling || !self.runs.iter().any(is_running) {
            return;
        }
        self.polling = true;
        ctx.task("poll", async {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            Ok(Value::Null)
        });
    }

    fn edit(&mut self, run: Option<&Value>, ctx: &mut Ctx) {
        self.draft = match run {
            Some(r) => Draft {
                id: r["id"].as_str().map(String::from),
                name: r["name"].as_str().unwrap_or_default().into(),
                command: r["command"].as_str().unwrap_or_default().into(),
                port: r["port"].as_u64().map(|p| p as u16),
            },
            None => Draft::default(),
        };
        ctx.prompt("form:name", "Name (e.g. dev)", self.draft.name.clone());
    }

    fn save_draft(&mut self, docker: bool, ctx: &mut Ctx) {
        let d = self.draft.clone();
        let body = json!({ "name": d.name, "command": d.command, "port": d.port, "docker": docker });
        match &d.id {
            Some(rid) => ctx.send("saved", Method::PATCH, self.path(&format!("/runs/{rid}")), body),
            None => ctx.send("saved", Method::POST, self.path("/runs"), body),
        }
    }
}

impl Screen for RunTab {
    fn title(&self) -> String {
        "Run".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        self.reload(ctx);
        ctx.get("suggest", self.path("/runs/suggest"));
    }

    fn captures_text(&self) -> bool {
        self.typing
    }

    fn actions(&self) -> Vec<Action> {
        let running = self.selected().is_some_and(is_running);
        vec![
            Action::new("r", if running { "Stop the selected command" } else { "Start the selected command" }),
            Action::new("p", "Start / stop the primary command"),
            Action::new("i", format!("Type into the output terminal ({LEAVE_HINT} leaves)")),
            Action::new("n", "Add a command"),
            Action::new("e", "Edit the selected command"),
            Action::new("x", "Delete the selected command"),
            Action::new("*", "Make the selected command primary"),
            Action::new("d", "Toggle Docker for the selected command"),
            Action::new("s", "Add a command suggested by the repository"),
            Action::new("v", "Preview the selected command in the browser"),
            Action::new("V", "Preview another port in the browser"),
            Action::new("R", "Reload"),
        ]
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        let sel = self.selected().cloned();
        match key {
            "r" => {
                if let Some(r) = sel {
                    self.toggle(r, ctx);
                }
            }
            "p" => match self.runs.iter().find(|r| r["primary"] == true).or(self.runs.first()).cloned() {
                Some(r) => {
                    self.select_id(&r["id"]);
                    self.toggle(r, ctx);
                }
                None => self.edit(None, ctx),
            },
            "i" => {
                if self.term.running() {
                    self.typing = true;
                } else {
                    ctx.info("the selected command is not running");
                }
            }
            "n" => self.edit(None, ctx),
            "e" => {
                if let Some(r) = sel {
                    self.edit(Some(&r), ctx);
                }
            }
            "x" => {
                if let Some(r) = sel {
                    ctx.confirm(
                        &format!("delete:{}", r["id"].as_str().unwrap_or_default()),
                        "Delete the command",
                        format!("Delete “{}”?", r["name"].as_str().unwrap_or_default()),
                        true,
                    );
                }
            }
            "*" => {
                if let Some(r) = sel.filter(|r| r["primary"] != true) {
                    let rid = r["id"].as_str().unwrap_or_default();
                    ctx.send("saved", Method::PATCH, self.path(&format!("/runs/{rid}")), json!({ "primary": true }));
                }
            }
            "d" => {
                if let Some(r) = sel {
                    let rid = r["id"].as_str().unwrap_or_default();
                    ctx.send("saved", Method::PATCH, self.path(&format!("/runs/{rid}")), json!({ "docker": r["docker"] != true }));
                }
            }
            "s" => {
                if self.suggestions.is_empty() {
                    ctx.info("no suggestions from the repository (package.json, Makefile, …)");
                    return;
                }
                let items = self
                    .suggestions
                    .iter()
                    .enumerate()
                    .map(|(i, sg)| {
                        let port = sg["port"].as_u64().map(|p| format!(" :{p}")).unwrap_or_default();
                        let docker = if sg["docker"] == true { " · docker" } else { "" };
                        (
                            i.to_string(),
                            format!(
                                "{}{port} — {}{docker} ({})",
                                sg["command"].as_str().unwrap_or_default(),
                                sg["name"].as_str().unwrap_or_default(),
                                sg["source"].as_str().unwrap_or_default()
                            ),
                        )
                    })
                    .collect();
                ctx.pick("suggestion", "Add a command from the repository", items);
            }
            "v" => {
                if let Some(r) = sel {
                    self.preview(&r, ctx);
                }
            }
            "V" => ctx.prompt("preview-port", "Preview a port (an app started in Shell, docker compose…)", ""),
            "R" => self.on_show(ctx),
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        if self.typing {
            if is_leave(&k) || !self.term.running() {
                self.typing = false;
            } else {
                self.term.key(k, ctx);
            }
            return Handled::Yes;
        }
        let n = self.runs.len();
        match k.code {
            KeyCode::Down | KeyCode::Char('j') if n > 0 => {
                self.list.select(Some((self.list.selected().unwrap_or(0) + 1).min(n - 1)));
                self.follow_selected(ctx);
            }
            KeyCode::Up | KeyCode::Char('k') if n > 0 => {
                self.list.select(Some(self.list.selected().unwrap_or(0).saturating_sub(1)));
                self.follow_selected(ctx);
            }
            KeyCode::Enter => {
                if self.term.running() {
                    self.typing = true;
                } else if let Some(r) = self.selected().cloned() {
                    self.start(&r, ctx);
                }
            }
            _ if n > 0 && crate::screens::kit::list_motion(&mut self.list, &k, n) => self.follow_selected(ctx),
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn on_paste(&mut self, text: &str, ctx: &mut Ctx) {
        if self.typing {
            self.term.input(text, ctx);
        }
    }

    fn on_stream(&mut self, key: &str, msg: Value, ctx: &mut Ctx) {
        if self.term.on_stream(key, &msg) == Some(true) {
            self.typing = false;
            self.reload(ctx);
        }
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        if self.term.on_task(key, &result, ctx) {
            return;
        }
        if key == "poll" {
            self.polling = false;
            self.reload(ctx);
            return;
        }
        let Some(v) = ctx.ok(result) else { return };
        match key {
            "runs" => {
                self.runs = v.as_array().cloned().unwrap_or_default();
                let n = self.runs.len();
                if n > 0 && self.list.selected().is_none_or(|i| i >= n) {
                    self.list.select(Some(n - 1));
                }
                if !self.loaded && n > 0 {
                    // first load: show the running one (as the web app does)
                    if let Some(i) = self.runs.iter().position(is_running) {
                        self.list.select(Some(i));
                    }
                    self.loaded = true;
                }
                self.follow_selected(ctx);
                self.schedule_poll(ctx);
            }
            "suggest" => self.suggestions = v.as_array().cloned().unwrap_or_default(),
            "started" => {
                ctx.info(format!("started {}", v["name"].as_str().unwrap_or_default()));
                self.select_id(&v["id"].clone());
                if v["job"].is_object() {
                    self.term.attach(&v["job"], ctx);
                }
                self.reload(ctx);
            }
            "stopped" | "deleted" => self.reload(ctx),
            "saved" => {
                self.select_id(&v["id"].clone());
                self.on_show(ctx);
            }
            "form:name" => {
                let Some(name) = v.as_str().map(str::trim).filter(|s| !s.is_empty()) else { return };
                self.draft.name = name.to_string();
                ctx.prompt("form:command", "Command (runs in the project folder)", self.draft.command.clone());
            }
            "form:command" => {
                let Some(cmd) = v.as_str().map(str::trim).filter(|s| !s.is_empty()) else { return };
                self.draft.command = cmd.to_string();
                ctx.prompt(
                    "form:port",
                    "Port for preview (empty: detect from the output)",
                    self.draft.port.map(|p| p.to_string()).unwrap_or_default(),
                );
            }
            "form:port" => {
                let Some(s) = v.as_str() else { return };
                if !s.trim().is_empty() && parse_port(s).is_none() {
                    ctx.error("the port must be a number from 1 to 65535");
                    return ctx.prompt("form:port", "Port for preview (empty: detect from the output)", s);
                }
                self.draft.port = parse_port(s);
                let docker =
                    self.draft.id.as_ref().and_then(|id| self.runs.iter().find(|r| r["id"] == id.as_str())).map(|r| r["docker"] == true);
                ctx.confirm(
                    "form:docker",
                    "Needs Docker?",
                    format!(
                        "Does the command need Docker (docker compose…)? It then runs without the sandbox — Docker access ≈ root on the server.{}",
                        match docker {
                            Some(true) => " Currently: yes.",
                            Some(false) => " Currently: no.",
                            None => "",
                        }
                    ),
                    false,
                );
            }
            "form:docker" => self.save_draft(v == Value::Bool(true), ctx),
            "suggestion" => {
                let Some(sg) = v.as_str().and_then(|i| i.parse::<usize>().ok()).and_then(|i| self.suggestions.get(i)).cloned() else {
                    return;
                };
                let body = json!({ "name": sg["name"], "command": sg["command"], "port": sg["port"], "docker": sg["docker"] == true });
                ctx.send("saved", Method::POST, self.path("/runs"), body);
            }
            "preview-port" => {
                let Some(s) = v.as_str() else { return };
                match parse_port(s) {
                    Some(port) => ctx.send("preview", Method::POST, self.path("/preview"), json!({ "port": port })),
                    None => ctx.error("the port must be a number from 1 to 65535"),
                }
            }
            "preview" => {
                if let Some(port) = v["previewPort"].as_u64() {
                    ctx.open_url(preview_url(&ctx.shared.base_url, port));
                }
            }
            k if k.starts_with("delete:") && v == Value::Bool(true) => {
                ctx.send("deleted", Method::DELETE, self.path(&format!("/runs/{}", &k[7..])), Value::Null);
            }
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx) {
        let t = Theme::get();
        let list_h = (self.runs.len() as u16 + 2).clamp(4, area.height / 2);
        let sugg_h = if self.suggestions.is_empty() { 0 } else { 1 };
        let [list_area, sugg_area, out_area] =
            Layout::vertical([Constraint::Length(list_h), Constraint::Length(sugg_h), Constraint::Min(4)]).areas(area);

        let items: Vec<ListItem> = self
            .runs
            .iter()
            .map(|r| {
                let (mark, color) = if is_running(r) {
                    ("●", t.ok)
                } else if r["job"]["exitCode"].as_i64().is_some_and(|c| c != 0) {
                    ("●", t.danger)
                } else {
                    ("○", t.muted)
                };
                let mut spans = vec![
                    Span::styled(format!(" {mark} "), Style::default().fg(color)),
                    Span::styled(if r["primary"] == true { "★ " } else { "  " }, Style::default().fg(t.warn)),
                    Span::styled(r["name"].as_str().unwrap_or_default().to_string(), t.bold()),
                ];
                if r["docker"] == true {
                    spans.push(Span::styled(" [docker]", t.muted_style()));
                }
                if let Some(p) = r["activePort"].as_u64().or(r["port"].as_u64()) {
                    spans.push(Span::styled(format!(" :{p}"), t.accent_style()));
                }
                if is_running(r) && r["previewPort"].is_u64() {
                    spans.push(Span::styled(" (v preview)", t.muted_style()));
                }
                spans.push(Span::styled(format!("   {}", r["command"].as_str().unwrap_or_default()), t.muted_style()));
                ListItem::new(Line::from(spans))
            })
            .collect();
        let title = "run commands — r start/stop · Enter output · n add · e edit · x delete · * primary · s suggestions";
        if self.runs.is_empty() {
            let hint = if self.suggestions.is_empty() {
                "No commands yet — n adds one (e.g. npm run dev)."
            } else {
                "No commands yet — n adds one, s picks one the repository suggests."
            };
            f.render_widget(Paragraph::new(Span::styled(hint, t.muted_style())).block(block(title, !self.typing)), list_area);
        } else {
            f.render_stateful_widget(
                List::new(items).block(block(title, !self.typing)).highlight_style(t.selected()),
                list_area,
                &mut self.list,
            );
        }
        if !self.suggestions.is_empty() {
            let cmds: Vec<String> = self.suggestions.iter().filter_map(|s| s["command"].as_str().map(String::from)).collect();
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(" s ", t.key_style()),
                    Span::styled(format!(" from the repository: {}", cmds.join(" · ")), t.muted_style()),
                ])),
                sugg_area,
            );
        }
        let name = self.selected().and_then(|r| r["name"].as_str()).unwrap_or("output").to_string();
        self.term.draw(
            f,
            out_area,
            &format!("output: {name}"),
            self.typing,
            "Start a command (r) to see its output here; Enter types into it while it runs.",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_urls() {
        assert_eq!(preview_url("http://127.0.0.1:8787", 41000), "http://127.0.0.1:41000/");
        assert_eq!(preview_url("https://hub.example.com/", 41001), "https://hub.example.com:41001/");
        assert_eq!(preview_url("http://user:pw@box.lan:8787/api", 5), "http://box.lan:5/");
        assert_eq!(preview_url("http://[::1]:8787", 7), "http://[::1]:7/");
    }

    #[test]
    fn ports() {
        assert_eq!(parse_port(" 5173 "), Some(5173));
        assert_eq!(parse_port("0"), None);
        assert_eq!(parse_port("70000"), None);
        assert_eq!(parse_port("abc"), None);
    }
}
