//! Harness → MCP (the web app's MCP page): MCP servers × agents, read from each agent's own
//! config file. Enter toggles the agent column under the cursor (connect copies the server's
//! definition from another agent, disconnect removes it from that agent's user config), `a`
//! adds a server through prompts, `x` removes it from every agent. Project servers and the
//! hub's built-in server are read-only.

use crossterm::event::{KeyCode, KeyEvent};
use hubness_client::Method;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, Paragraph, Wrap};
use serde_json::{Map, Value, json};

use super::harness_ext::tilde;
use super::kit::{Nav, click_items, click_select, clicked_row, s};
use super::session::enc;
use crate::core::{Action, Ctx, Handled, Hit, Screen};
use crate::ui::theme::icon;
use crate::ui::{Theme, block};

/// A server being added through prompts.
#[derive(Default, Clone)]
struct Draft {
    name: String,
    transport: String,
    command: String,
    args: Vec<String>,
    pairs: Map<String, Value>,
    url: String,
}

#[derive(Default)]
pub struct McpTab {
    data: Value,
    error: Option<String>,
    nav: Nav,
    /// The agent column under the cursor.
    col: usize,
    /// A project whose own config is shown too: (id, name).
    project: Option<(String, String)>,
    draft: Draft,
}

const NAME_W: usize = 26;
const CELL_W: usize = 13;

impl McpTab {
    pub fn new() -> Self {
        Self::default()
    }

    fn query(&self) -> String {
        self.project.as_ref().map(|(id, _)| format!("?project={}", enc(id))).unwrap_or_default()
    }

    fn load(&self, ctx: &Ctx) {
        ctx.get("mcp", format!("/harness/mcp{}", self.query()));
    }

    fn servers(&self) -> Vec<Value> {
        self.data["servers"].as_array().cloned().unwrap_or_default()
    }

    fn agents(&self) -> Vec<Value> {
        self.data["agents"].as_array().cloned().unwrap_or_default()
    }

    fn selected(&self) -> Option<Value> {
        self.nav.selected().and_then(|i| self.servers().get(i).cloned())
    }

    fn move_col(&mut self, d: isize) {
        let n = self.agents().len();
        if n > 0 {
            self.col = (self.col as isize + d).rem_euclid(n as isize) as usize;
        }
    }

    /// Connect or disconnect the server under the cursor in the agent column under the cursor.
    fn toggle(&mut self, ctx: &mut Ctx) {
        let (Some(server), Some(agent)) = (self.selected(), self.agents().get(self.col).cloned()) else { return };
        let (name, kind) = (s(&server["name"], ""), s(&agent["kind"], ""));
        let label = s(&agent["label"], kind);
        match server["agents"][kind].as_str() {
            Some("builtin") => ctx.info("the hub attaches its built-in server to every hub session by itself"),
            Some("project") => {
                let src = server["entries"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|e| e["agent"] == kind)
                    .map(|e| s(&e["source"], ""))
                    .unwrap_or("");
                ctx.info(format!("{name} comes from the project config {src} — edit that file"));
            }
            Some(_) => {
                ctx.info(format!("disconnecting {name} from {label}…"));
                ctx.send(
                    "changed",
                    Method::DELETE,
                    format!("/harness/mcp/{}/agents/{}{}", enc(name), enc(kind), self.query()),
                    Value::Null,
                );
            }
            None => {
                if let Some(e) = agent["error"].as_str() {
                    return ctx.error(e.to_string());
                }
                ctx.info(format!("connecting {name} to {label}…"));
                ctx.send("changed", Method::PUT, format!("/harness/mcp/{}/agents/{}{}", enc(name), enc(kind), self.query()), Value::Null);
            }
        }
    }

    fn submit(&mut self, kinds: Vec<String>, ctx: &mut Ctx) {
        let d = std::mem::take(&mut self.draft);
        let body = if d.transport == "stdio" {
            json!({ "name": d.name, "transport": "stdio", "command": d.command, "args": d.args, "env": d.pairs, "agents": kinds })
        } else {
            json!({ "name": d.name, "transport": d.transport, "url": d.url, "headers": d.pairs, "agents": kinds })
        };
        ctx.info(format!("adding {}…", d.name));
        ctx.send("changed", Method::POST, format!("/harness/mcp{}", self.query()), body);
    }

    /// `KEY=value` / `Name: value` pairs separated by `;`.
    fn pairs(text: &str, sep: char) -> Result<Map<String, Value>, String> {
        let mut out = Map::new();
        for part in text.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            let (k, v) = part.split_once(sep).ok_or_else(|| format!("not a pair: {part}"))?;
            out.insert(k.trim().to_string(), json!(v.trim()));
        }
        Ok(out)
    }

    fn cell(&self, server: &Value, agent: &Value) -> (String, Style) {
        let t = Theme::get();
        let kind = s(&agent["kind"], "");
        let entry = server["entries"].as_array().into_iter().flatten().find(|e| e["agent"] == kind);
        match server["agents"][kind].as_str() {
            Some("builtin") => (format!("{} built in", icon("\u{f0e7}", "*")), t.muted_style()),
            Some("project") => (format!("{} project", icon("\u{f07b}", "+")), t.accent_style()),
            Some(_) if entry.is_some_and(|e| e["enabled"] == false) => {
                (format!("{} off", icon("\u{f205}", "[x]")), Style::default().fg(t.warn))
            }
            Some(_) => (format!("{} on", icon("\u{f205}", "[x]")), Style::default().fg(t.ok)),
            None if !agent["transports"].as_array().is_some_and(|t| t.contains(&server["transport"])) => ("  n/a".into(), t.muted_style()),
            None => (format!("{} ·", icon("\u{f204}", "[ ]")), t.muted_style()),
        }
    }

    fn details(&self, server: &Value) -> Vec<Line<'static>> {
        let t = Theme::get();
        let mut l = vec![];
        if server["builtIn"] == true {
            l.push(Line::raw(" The hub's own server (open_pane, close_pane), attached to every hub session by the hub."));
            return l;
        }
        let target = if server["transport"] == "stdio" {
            let mut parts = vec![s(&server["command"], "").to_string()];
            parts.extend(server["args"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(String::from)));
            parts.join(" ")
        } else {
            s(&server["url"], "").to_string()
        };
        l.push(Line::from(vec![
            Span::styled(format!(" {} ", s(&server["transport"], "").to_uppercase()), t.muted_style()),
            Span::raw(target),
        ]));
        let keys_of = |k: &str| server[k].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<_>>();
        let keys = if server["transport"] == "stdio" { keys_of("env") } else { keys_of("headers") };
        if !keys.is_empty() {
            l.push(Line::styled(
                format!(
                    " {}: {} (values stay on the server)",
                    if server["transport"] == "stdio" { "env" } else { "headers" },
                    keys.join(", ")
                ),
                t.muted_style(),
            ));
        }
        for e in server["entries"].as_array().into_iter().flatten() {
            let off = if e["enabled"] == false { "  (turned off)" } else { "" };
            l.push(Line::styled(
                format!(" {:<12} {:<8} {}{off}", s(&e["agent"], ""), s(&e["scope"], ""), tilde(s(&e["source"], ""))),
                t.muted_style(),
            ));
        }
        l
    }
}

impl Screen for McpTab {
    fn title(&self) -> String {
        "MCP".into()
    }

    fn on_show(&mut self, ctx: &mut Ctx) {
        self.load(ctx);
    }

    fn actions(&self) -> Vec<Action> {
        let mut a = vec![
            Action::new("a", "Add an MCP server"),
            Action::new("r", "Reload"),
            Action::new("P", "Show a project's servers too"),
            Action::new("[", "Previous agent column"),
            Action::new("]", "Next agent column"),
        ];
        if self.selected().is_some_and(|s| s["builtIn"] != true) {
            a.insert(1, Action::new("x", "Remove the server from every agent"));
        }
        a
    }

    fn on_action(&mut self, key: &str, ctx: &mut Ctx) {
        match key {
            "a" => {
                self.draft = Draft::default();
                ctx.prompt("add-name", "MCP server name (letters, digits, - and _)", "");
            }
            "r" => self.load(ctx),
            "x" => {
                if let Some(srv) = self.selected().filter(|s| s["builtIn"] != true) {
                    let name = s(&srv["name"], "").to_string();
                    ctx.confirm(
                        &format!("delete:{name}"),
                        "Remove MCP server",
                        format!("Remove {name} from every agent's user config?"),
                        true,
                    );
                }
            }
            "P" => {
                let mut items = vec![("".to_string(), "No project — user configs only".to_string())];
                items.extend(
                    ctx.shared
                        .projects
                        .iter()
                        .filter(|p| p["kind"] == "normal")
                        .map(|p| (format!("{}\u{1f}{}", s(&p["id"], ""), s(&p["name"], "")), s(&p["name"], "").to_string())),
                );
                ctx.pick("project", "Show a project's MCP servers too", items);
            }
            "[" => self.move_col(-1),
            "]" => self.move_col(1),
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Handled {
        let n = self.servers().len();
        if self.nav.key(&k, n) {
            return Handled::Yes;
        }
        match k.code {
            KeyCode::Left | KeyCode::BackTab => self.move_col(-1),
            KeyCode::Right | KeyCode::Tab => self.move_col(1),
            KeyCode::Enter => self.toggle(ctx),
            KeyCode::Delete => self.on_action("x", ctx),
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn on_click(&mut self, id: &str, ctx: &mut Ctx) {
        if let Some(rest) = id.strip_prefix("cell:")
            && let Some((r, c)) = rest.split_once(':')
            && let (Ok(r), Ok(c)) = (r.parse::<usize>(), c.parse::<usize>())
        {
            let again = self.nav.selected() == Some(r) && self.col == c;
            self.nav.state.select(Some(r));
            self.col = c;
            if again {
                self.toggle(ctx);
            }
        } else if let Some(c) = id.strip_prefix("col:").and_then(|c| c.parse().ok()) {
            self.col = c;
        } else if let Some(i) = clicked_row(id, "rows") {
            click_select(&mut self.nav.state, i);
        }
    }

    fn on_task(&mut self, key: &str, result: anyhow::Result<Value>, ctx: &mut Ctx) {
        match key {
            "mcp" => match result {
                Ok(v) => {
                    self.data = v;
                    self.error = None;
                }
                Err(e) => self.error = Some(e.to_string()),
            },
            "changed" => {
                if let Some(v) = ctx.ok(result) {
                    self.data = v;
                    ctx.info("saved to the agents' config files");
                }
            }
            "project" => {
                if let Ok(Value::String(id)) = result {
                    self.project = id.split_once('\u{1f}').map(|(i, n)| (i.to_string(), n.to_string()));
                    self.load(ctx);
                }
            }
            k if k.starts_with("delete:") => {
                if matches!(result, Ok(Value::Bool(true))) {
                    let name = &k["delete:".len()..];
                    ctx.send("changed", Method::DELETE, format!("/harness/mcp/{}{}", enc(name), self.query()), Value::Null);
                }
            }
            "add-name" => {
                let Ok(Value::String(name)) = result else { return };
                let name = name.trim().to_string();
                if name.is_empty() {
                    return;
                }
                self.draft.name = name;
                let items = vec![
                    ("stdio".to_string(), "stdio — a local command".to_string()),
                    ("http".to_string(), "HTTP — a remote server (streamable HTTP)".to_string()),
                    ("sse".to_string(), "SSE — a remote server (server-sent events)".to_string()),
                ];
                ctx.pick("add-transport", format!("{} — transport", self.draft.name), items);
            }
            "add-transport" => {
                let Ok(Value::String(tr)) = result else { return };
                self.draft.transport = tr.clone();
                if tr == "stdio" {
                    ctx.prompt("add-command", "Command and arguments (split on spaces)", "");
                } else {
                    ctx.prompt("add-url", "URL (https://…)", "");
                }
            }
            "add-command" => {
                let Ok(Value::String(cmd)) = result else { return };
                let mut words = cmd.split_whitespace().map(String::from);
                self.draft.command = words.next().unwrap_or_default();
                self.draft.args = words.collect();
                ctx.prompt("add-pairs", "Environment: KEY=value; KEY2=value (optional)", "");
            }
            "add-url" => {
                let Ok(Value::String(url)) = result else { return };
                self.draft.url = url.trim().to_string();
                ctx.prompt("add-pairs", "Headers: Name: value; Name2: value (optional)", "");
            }
            "add-pairs" => {
                let Ok(Value::String(text)) = result else { return };
                match Self::pairs(&text, if self.draft.transport == "stdio" { '=' } else { ':' }) {
                    Ok(p) => self.draft.pairs = p,
                    Err(e) => return ctx.error(e),
                }
                let usable: Vec<String> = self
                    .agents()
                    .iter()
                    .filter(|a| {
                        a["supported"] == true
                            && a["error"].is_null()
                            && a["transports"].as_array().is_some_and(|t| t.iter().any(|x| x == self.draft.transport.as_str()))
                    })
                    .map(|a| s(&a["kind"], "").to_string())
                    .collect();
                ctx.prompt("add-agents", "Add to agents (comma-separated)", usable.join(", "));
            }
            "add-agents" => {
                let Ok(Value::String(text)) = result else { return };
                let kinds: Vec<String> = text.split([',', ' ']).map(str::trim).filter(|k| !k.is_empty()).map(String::from).collect();
                self.submit(kinds, ctx);
            }
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let t = Theme::get();
        let servers = self.servers();
        let agents = self.agents();
        let [hint, head, body, detail] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(4), Constraint::Length(10)]).areas(area);
        let project = self.project.as_ref().map(|(_, n)| format!("   project: {n}")).unwrap_or_default();
        f.render_widget(
            Paragraph::new(Line::styled(
                format!(" Written to each agent's own config — the agent sees them outside the hub too.{project}"),
                t.muted_style(),
            )),
            hint,
        );
        if self.data.is_null() {
            let msg = self.error.clone().unwrap_or_else(|| "Loading…".into());
            f.render_widget(Paragraph::new(msg).wrap(Wrap { trim: false }).block(block("MCP servers", true)), body);
            return;
        }
        self.col = self.col.min(agents.len().saturating_sub(1));
        // the column header: agent names, the one under the cursor highlighted
        let mut spans = vec![Span::raw(format!(" {:<NAME_W$}", "server"))];
        let mut x = head.x + 1 + NAME_W as u16;
        for (i, a) in agents.iter().enumerate() {
            let label: String = s(&a["label"], "").chars().take(CELL_W - 1).collect();
            let style = if i == self.col {
                t.key_style()
            } else if a["error"].is_null() {
                t.dim_style()
            } else {
                Style::default().fg(t.danger)
            };
            spans.push(Span::styled(format!("{label:<w$}", w = CELL_W - 1), style));
            spans.push(Span::raw(" "));
            ctx.click(
                Rect { x, y: head.y, width: (CELL_W as u16).min((head.x + head.width).saturating_sub(x)), height: 1 },
                Hit::Click(format!("col:{i}")),
            );
            x += CELL_W as u16;
        }
        f.render_widget(Paragraph::new(Line::from(spans)), head);

        self.nav.clamp(servers.len());
        let selected = self.nav.selected();
        let items: Vec<ListItem> = servers
            .iter()
            .enumerate()
            .map(|(r, srv)| {
                let name = if srv["builtIn"] == true { "hubness (built in)".to_string() } else { s(&srv["name"], "").to_string() };
                let name: String = name.chars().take(NAME_W - 1).collect();
                let mut spans = vec![Span::styled(format!("{name:<NAME_W$}"), t.bold())];
                for (c, a) in agents.iter().enumerate() {
                    let (text, mut style) = self.cell(srv, a);
                    if selected == Some(r) && c == self.col {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                    spans.push(Span::styled(format!("{text:<w$}", w = CELL_W - 1), style));
                    spans.push(Span::raw(" "));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();
        let title = "MCP servers — Enter toggle · [ ] column · a add · x remove · P project";
        let b = block(title, true);
        let inner = b.inner(body);
        f.render_stateful_widget(List::new(items).block(b).highlight_style(Style::default().bg(t.selection)), body, &mut self.nav.state);
        click_items(ctx, "rows", body, true, &self.nav.state, &vec![1; servers.len()]);
        // cells: a click picks the cell, a second click toggles it
        for row in 0..(inner.height as usize).min(servers.len().saturating_sub(self.nav.state.offset())) {
            let r = self.nav.state.offset() + row;
            for c in 0..agents.len() {
                let cx = inner.x + NAME_W as u16 + (c * CELL_W) as u16;
                if cx >= inner.x + inner.width {
                    break;
                }
                let w = (CELL_W as u16).min(inner.x + inner.width - cx);
                ctx.click(Rect { x: cx, y: inner.y + row as u16, width: w, height: 1 }, Hit::Click(format!("cell:{r}:{c}")));
            }
        }

        let mut lines = selected.and_then(|i| servers.get(i)).map(|srv| self.details(srv)).unwrap_or_default();
        for a in &agents {
            if let Some(e) = a["error"].as_str() {
                lines.push(Line::styled(format!(" {}: {e}", s(&a["label"], "")), Style::default().fg(t.danger)));
            }
        }
        if let Some(a) = agents.get(self.col) {
            lines.push(Line::styled(format!(" {} config: {}", s(&a["label"], ""), tilde(s(&a["configPath"], "—"))), t.muted_style()));
        }
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block("details", false)), detail);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_parse() {
        let m = McpTab::pairs("A=1; B = two", '=').expect("pairs");
        assert_eq!(m["A"], "1");
        assert_eq!(m["B"], "two");
        assert!(McpTab::pairs("", ':').expect("empty").is_empty());
        assert!(McpTab::pairs("nope", ':').is_err());
    }
}
