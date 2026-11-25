//! Shared ACP (Agent Client Protocol) client — a port of `agents/acp.ts`: JSON-RPC 2.0 over
//! stdio. Turn: initialize → session/new | session/load (resume) → model and mode →
//! session/prompt. Streaming via session/update notifications, permissions via
//! session/request_permission requests.
//!
//! Everything that differs between agents (launch command, how to pass MCP, how to enable
//! no-prompt mode) is described in `Spec`; the client itself is agent-agnostic. Codex,
//! OpenCode and Grok specs are at the bottom.
//!

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use hubness_core::{PermissionMode, SessionStatus};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

use super::{ApprovalDecision, Decision, RunHandle, Started, StopReason, kill_group, stop_notice};
use crate::config::child_env;
use crate::runtime::Turn;
use crate::sessions::truncate;

const OUTPUT_LIMIT: usize = 20_000;

type InfoFn = fn(&Value) -> Option<(String, String)>;
/// (cmd, args, extra env) that launches an agent's ACP server.
type Launch = (String, Vec<String>, Vec<(String, String)>);

/// What differs between ACP agents.
pub struct Spec {
    pub kind: &'static str,
    /// Name used in error messages.
    pub label: &'static str,
    /// Command that launches the agent's ACP server: (cmd, args, extra env).
    pub command: fn(&Turn, PermissionMode) -> Launch,
    /// Pass hub MCP servers as the ACP mcpServers parameter (otherwise via command/env).
    pub mcp_via_param: bool,
    /// `_meta` for session/new and session/load (e.g. Grok's yolo mode).
    pub session_meta: Option<fn(PermissionMode) -> Value>,
    /// Agent mode id (session/set_mode) for our permission mode, if the agent declares it.
    pub mode_ids: &'static [(PermissionMode, &'static str)],
    /// The agent applies permission modes itself (via config); no client-side policy.
    pub native_permissions: bool,
    /// session_info_update → (retry line for the feed, provider error), or `None`.
    pub describe_info: Option<InfoFn>,
    /// OpenCode: switch the `mode` config option after the session is ready.
    pub mode_config_option: bool,
}

/// Models the agent declared in the last session (for selection in the UI).
pub fn models_key(kind: &str) -> String {
    format!("models:{kind}")
}

fn remember_models(turn: &Turn, kind: &str, r: &Value) {
    let from_config: Option<Vec<Value>> =
        r["configOptions"].as_array().and_then(|opts| opts.iter().find(|o| o["id"] == "model")).and_then(|opt| {
            opt["options"].as_array().map(|os| {
                os.iter()
                    .flat_map(|o| match o["options"].as_array() {
                        Some(inner) => inner.iter().map(|x| x["value"].clone()).collect::<Vec<_>>(),
                        None => vec![o["value"].clone()],
                    })
                    .collect()
            })
        });
    let from_models: Option<Vec<Value>> =
        r["models"]["availableModels"].as_array().map(|ms| ms.iter().map(|m| m["modelId"].clone()).collect());
    let list = from_config.filter(|l| !l.is_empty()).or(from_models);
    let list: Vec<Value> = list.unwrap_or_default().into_iter().filter(Value::is_string).collect();
    if !list.is_empty() {
        turn.kv_set(&models_key(kind), &Value::Array(list));
    }
}

fn current_model(r: &Value) -> Option<String> {
    let from_config =
        r["configOptions"].as_array().and_then(|opts| opts.iter().find(|o| o["id"] == "model")).and_then(|o| o["currentValue"].as_str());
    from_config.or_else(|| r["models"]["currentModelId"].as_str()).map(String::from)
}

// ---------- tool normalization ----------

/// JS truthiness of a JSON value.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        _ => true,
    }
}

/// The first of `keys` that is present and not null (JS `a ?? b ?? c`).
fn pick<'a>(raw: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| raw.get(*k).filter(|v| !v.is_null()))
}

/// An object from (key, value) pairs; absent values are left out (like JS `undefined`).
fn obj(pairs: Vec<(&str, Option<Value>)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        if let Some(v) = v {
            m.insert(k.to_string(), v);
        }
    }
    Value::Object(m)
}

/// Agent tools → canonical names. `title` can be the tool name (write) or the command
/// itself (bash), so the input fields are looked at first.
fn detect_tool(title: &str, raw: &Value) -> String {
    let t = title.to_lowercase();
    if ["write", "edit", "read", "bash", "grep", "glob", "webfetch", "todowrite", "task", "list"].contains(&t.as_str()) {
        return t;
    }
    if raw["command"].is_string() || raw["command"].is_array() {
        return "bash".into();
    }
    if raw.get("oldString").is_some() || raw.get("old_string").is_some() {
        return "edit".into();
    }
    if raw.get("content").is_some() && truthy(pick(raw, &["filePath", "path", "file_path"])) {
        return "write".into();
    }
    if raw["todos"].is_array() {
        return "todowrite".into();
    }
    if truthy(raw.get("url")) {
        return "webfetch".into();
    }
    if truthy(raw.get("pattern")) {
        return if truthy(raw.get("include")) || truthy(raw.get("path")) { "grep" } else { "glob" }.into();
    }
    if truthy(pick(raw, &["filePath", "file_path"])) {
        return "read".into();
    }
    t
}

pub fn normalize_tool(title: &str, raw: &Value) -> (String, Value) {
    // Grok calls MCP tools through a meta-tool: search and call by name
    if raw["variant"] == "UseTool"
        && let Some(name) = raw["tool_name"].as_str()
    {
        let input = raw.get("tool_input").filter(|v| !v.is_null()).cloned().unwrap_or_else(|| json!({}));
        return normalize_tool(name, &input);
    }
    if raw["variant"] == "SearchTool" {
        return ("ToolSearch".into(), obj(vec![("query", raw.get("query").cloned())]));
    }
    if raw["variant"] == "ListDir" {
        return ("LS".into(), obj(vec![("path", raw.get("target_directory").cloned())]));
    }
    // hub MCP tools: the same names as in Claude Code
    let pane_kind = raw["kind"].as_str().is_some_and(|k| ["url", "file", "diff"].contains(&k));
    if title.contains("open_pane") || (truthy(raw.get("kind")) && truthy(raw.get("target")) && pane_kind) {
        return ("mcp__hub__open_pane".into(), raw.clone());
    }
    if title.contains("close_pane") {
        return ("mcp__hub__close_pane".into(), raw.clone());
    }
    let path = pick(raw, &["filePath", "path", "file_path"]).cloned();
    let get = |k: &str| raw.get(k).cloned();
    match detect_tool(title, raw).as_str() {
        "write" => ("Write".into(), obj(vec![("file_path", path), ("content", get("content"))])),
        "edit" => (
            "Edit".into(),
            obj(vec![
                ("file_path", path),
                ("old_string", pick(raw, &["oldString", "old_string"]).cloned()),
                ("new_string", pick(raw, &["newString", "new_string"]).cloned()),
            ]),
        ),
        "read" => ("Read".into(), obj(vec![("file_path", path)])),
        "bash" => {
            let command = match &raw["command"] {
                Value::Array(parts) => Some(json!(
                    parts.iter().map(|p| p.as_str().map(String::from).unwrap_or_else(|| p.to_string())).collect::<Vec<_>>().join(" ")
                )),
                Value::Null => None,
                other => Some(other.clone()),
            };
            ("Bash".into(), obj(vec![("command", command), ("description", get("description"))]))
        }
        "grep" => ("Grep".into(), obj(vec![("pattern", get("pattern")), ("path", get("path"))])),
        "glob" => ("Glob".into(), obj(vec![("pattern", get("pattern"))])),
        "webfetch" => ("WebFetch".into(), obj(vec![("url", get("url"))])),
        "todowrite" => ("TodoWrite".into(), obj(vec![("todos", get("todos"))])),
        "task" => ("Task".into(), obj(vec![("description", pick(raw, &["description", "prompt"]).cloned())])),
        _ => {
            let name = if !title.is_empty() && title.encode_utf16().count() <= 40 { title.to_string() } else { "Tool".into() };
            (name, raw.clone())
        }
    }
}

/// ACP content of type diff → an edit in our format (if there is no tool input).
fn diff_tool(content: &Value) -> Option<(String, Value)> {
    let d = content.as_array()?.iter().find(|c| c["type"] == "diff")?;
    Some(if truthy(d.get("oldText")) {
        (
            "Edit".into(),
            obj(vec![
                ("file_path", d.get("path").cloned()),
                ("old_string", d.get("oldText").cloned()),
                ("new_string", d.get("newText").cloned()),
            ]),
        )
    } else {
        ("Write".into(), obj(vec![("file_path", d.get("path").cloned()), ("content", d.get("newText").cloned())]))
    })
}

fn content_text(content: &Value) -> String {
    let Some(items) = content.as_array() else { return String::new() };
    let s = items
        .iter()
        .map(|c| {
            if c["type"] == "content" && c["content"]["type"] == "text" {
                c["content"]["text"].as_str().unwrap_or_default().to_string()
            } else if c["type"] == "diff" {
                format!("diff {}", c["path"].as_str().map(String::from).unwrap_or_else(|| "undefined".into()))
            } else {
                String::new()
            }
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if s.encode_utf16().count() > OUTPUT_LIMIT { format!("{}\n… (truncated)", truncate(&s, OUTPUT_LIMIT)) } else { s }
}

const WRITE_KINDS: &[&str] = &["edit", "delete", "move"];

/// Client-side policy for agents that have no modes of their own.
fn auto_decision(mode: PermissionMode, tool_kind: Option<&str>) -> Option<Decision> {
    if mode == PermissionMode::Bypass {
        return Some(Decision::Allow);
    }
    let writes = tool_kind.is_some_and(|k| WRITE_KINDS.contains(&k));
    match mode {
        PermissionMode::AcceptEdits if writes => Some(Decision::Allow),
        PermissionMode::Plan if writes => Some(Decision::Deny),
        _ => None,
    }
}

/// Session history recap for an agent that can't load its own sessions.
fn transcript(history: &[Value]) -> String {
    let lines: Vec<String> = history
        .iter()
        .filter_map(|e| match e["kind"].as_str() {
            Some("user") => Some(format!("User: {}", e["text"].as_str().unwrap_or_default())),
            Some("text") if !e["text"].as_str().unwrap_or_default().trim().is_empty() => {
                Some(format!("Assistant: {}", e["text"].as_str().unwrap_or_default()))
            }
            _ => None,
        })
        .collect();
    let s = lines.join("\n\n");
    let units: Vec<u16> = s.encode_utf16().collect();
    if units.len() > 30_000 { format!("…\n{}", String::from_utf16_lossy(&units[units.len() - 30_000..])) } else { s }
}

fn base36(mut n: u64) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = vec![];
    loop {
        out.push(D[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).expect("ascii")
}

// ---------- the client ----------

struct Block {
    uid: String,
    text: String,
}

#[derive(Default)]
struct State {
    session_id: Option<String>,
    replaying: bool,
    stopped: bool,
    stop_reason: Option<StopReason>,
    finished: bool,
    settled: bool,
    last_provider_error: Option<String>,
    model: Option<String>,
    text_block: Option<Block>,
    thought_block: Option<Block>,
    block_n: u64,
    /// jsonrpc id of a permission request → its params.
    pending: Vec<(i64, Value)>,
    tool_input: HashMap<String, (String, Value)>,
}

type Calls = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, Value>>>>>;

struct Client {
    spec: &'static Spec,
    turn: Arc<Turn>,
    state: Mutex<State>,
    out: mpsc::UnboundedSender<String>,
    calls: Calls,
    next_id: AtomicI64,
    run_tag: String,
    mode: PermissionMode,
    pid: Option<u32>,
}

impl Client {
    fn send(&self, v: Value) {
        let _ = self.out.send(v.to_string());
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        self.calls.lock().expect("acp calls").insert(id, tx);
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        rx.await.unwrap_or_else(|_| Err(json!({ "message": format!("{} exited", self.spec.label) })))
    }

    fn flush_text(&self, st: &mut State) {
        if let Some(b) = st.text_block.take() {
            self.turn.put(&b.uid, json!({ "kind": "text", "text": b.text, "streaming": false }), true);
        }
    }

    fn flush_thought(&self, st: &mut State) {
        if let Some(b) = st.thought_block.take()
            && !b.text.trim().is_empty()
        {
            self.turn.put(&b.uid, json!({ "kind": "thinking", "text": b.text }), true);
        }
    }

    fn on_update(&self, u: &Value) {
        let mut st = self.state.lock().expect("acp state");
        if st.replaying {
            return;
        }
        match u["sessionUpdate"].as_str().unwrap_or_default() {
            "agent_message_chunk" => {
                self.flush_thought(&mut st);
                let piece = if u["content"]["type"] == "text" { u["content"]["text"].as_str().unwrap_or_default() } else { "" };
                if st.text_block.is_none() {
                    st.block_n += 1;
                    st.text_block = Some(Block { uid: format!("t:{}:{}", self.run_tag, st.block_n), text: String::new() });
                }
                let b = st.text_block.as_mut().expect("just set");
                b.text.push_str(piece);
                self.turn.put(&b.uid, json!({ "kind": "text", "text": b.text, "streaming": true }), false);
            }
            "agent_thought_chunk" => {
                self.flush_text(&mut st);
                if st.thought_block.is_none() {
                    st.block_n += 1;
                    st.thought_block = Some(Block { uid: format!("th:{}:{}", self.run_tag, st.block_n), text: String::new() });
                }
                st.thought_block.as_mut().expect("just set").text.push_str(u["content"]["text"].as_str().unwrap_or_default());
            }
            "tool_call" | "tool_call_update" => {
                self.flush_text(&mut st);
                self.flush_thought(&mut st);
                let id = u["toolCallId"].as_str().map(String::from).unwrap_or_else(|| u["toolCallId"].to_string());
                let uid = format!("tool:{id}");
                let prev = self.turn.get(&uid).filter(|p| p["kind"] == "tool");
                let has_input = u["rawInput"].as_object().is_some_and(|o| !o.is_empty());
                let title = u.get("title").filter(|t| !t.is_null()).and_then(Value::as_str);
                let mut norm = if has_input {
                    let t = title.map(String::from).unwrap_or_else(|| {
                        prev.as_ref().map(|p| p["name"].as_str().unwrap_or_default().to_lowercase()).unwrap_or_default()
                    });
                    normalize_tool(&t, &u["rawInput"])
                } else {
                    st.tool_input
                        .get(&id)
                        .cloned()
                        .or_else(|| diff_tool(&u["content"]))
                        .unwrap_or_else(|| normalize_tool(title.unwrap_or_default(), &json!({})))
                };
                // input has no path/edit text but there is a diff: take the edit from it
                let diff = diff_tool(&u["content"]);
                if let Some(d) = &diff
                    && (norm.0 == "Edit" || norm.0 == "Write")
                    && !truthy(norm.1.get("new_string"))
                    && norm.1.get("content").is_none()
                {
                    norm = d.clone();
                }
                let known = has_input || st.tool_input.contains_key(&id) || diff.is_some();
                if known {
                    st.tool_input.insert(id.clone(), norm.clone());
                }
                let prev_status = prev.as_ref().map(|p| p["status"].clone());
                let status = match u["status"].as_str() {
                    Some("completed") => json!("ok"),
                    Some("failed") => json!("error"),
                    _ => prev_status.clone().unwrap_or_else(|| json!("running")),
                };
                let text = content_text(&u["content"]);
                let output = if !text.is_empty() { Some(json!(text)) } else { prev.as_ref().and_then(|p| p.get("output").cloned()) };
                let keep_prev = prev.is_some() && !known;
                let (name, input) = match (&prev, keep_prev) {
                    (Some(p), true) => (p["name"].clone(), p.get("input").cloned()),
                    _ => (json!(norm.0), Some(norm.1)),
                };
                let status = if prev_status.as_ref().is_some_and(|s| s == "denied") { json!("denied") } else { status };
                let body = obj(vec![
                    ("kind", Some(json!("tool"))),
                    ("toolId", Some(u["toolCallId"].clone())),
                    ("name", Some(name)),
                    ("input", input),
                    ("status", Some(status)),
                    ("output", output),
                ]);
                self.turn.put(&uid, body, true);
            }
            "session_info_update" => {
                if let Some((retry, error)) = self.spec.describe_info.and_then(|f| f(u)) {
                    st.last_provider_error = Some(error);
                    self.turn.put(&format!("retry:{}", self.run_tag), json!({ "kind": "notice", "level": "info", "text": retry }), true);
                }
            }
            "plan" => {
                let todos: Vec<Value> = u["entries"]
                    .as_array()
                    .map(|es| es.iter().map(|e| json!({ "content": e["content"], "status": e["status"] })).collect())
                    .unwrap_or_default();
                let tag = format!("plan:{}", self.run_tag);
                self.turn.put(
                    &tag,
                    json!({ "kind": "tool", "toolId": tag, "name": "TodoWrite", "input": { "todos": todos }, "status": "ok" }),
                    true,
                );
            }
            _ => {}
        }
    }

    fn answer(&self, id: i64, params: &Value, decision: Decision) {
        let want: &[&str] = if decision == Decision::Allow { &["allow_once", "allow_always"] } else { &["reject_once", "reject_always"] };
        let options = params["options"].as_array().cloned().unwrap_or_default();
        let opt = want.iter().find_map(|k| options.iter().find(|o| o["kind"] == *k));
        let result = match opt {
            Some(o) => json!({ "outcome": { "outcome": "selected", "optionId": o["optionId"] } }),
            None => json!({ "outcome": { "outcome": "cancelled" } }),
        };
        self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        if decision == Decision::Deny
            && let Some(tc) = params["toolCall"]["toolCallId"].as_str()
        {
            self.turn.patch(&format!("tool:{tc}"), json!({ "status": "denied" }), true);
        }
    }

    fn on_permission(&self, id: i64, params: Value) {
        let tc = &params["toolCall"];
        let mut st = self.state.lock().expect("acp state");
        if st.replaying {
            drop(st);
            return self.answer(id, &params, Decision::Deny);
        }
        let tc_id = tc["toolCallId"].as_str().unwrap_or_default().to_string();
        let norm = if tc["rawInput"].as_object().is_some_and(|o| !o.is_empty()) {
            let title = st
                .tool_input
                .get(&tc_id)
                .map(|t| t.0.to_lowercase())
                .or_else(|| tc["title"].as_str().map(String::from))
                .or_else(|| tc["kind"].as_str().map(String::from))
                .unwrap_or_default();
            normalize_tool(&title, &tc["rawInput"])
        } else {
            st.tool_input
                .get(&tc_id)
                .cloned()
                .or_else(|| diff_tool(&tc["content"]))
                .unwrap_or_else(|| (tc["title"].as_str().unwrap_or("tool").to_string(), json!({})))
        };
        // hub tools (pane) don't change anything: like Claude Code, no prompt
        if norm.0.starts_with("mcp__hub__") {
            drop(st);
            return self.answer(id, &params, Decision::Allow);
        }
        if !self.spec.native_permissions
            && let Some(d) = auto_decision(self.mode, tc["kind"].as_str())
        {
            drop(st);
            return self.answer(id, &params, d);
        }
        self.flush_text(&mut st);
        st.pending.push((id, params.clone()));
        drop(st);
        let rid = format!("{id}:{}", self.run_tag);
        self.turn.put(
            &format!("ap:{rid}"),
            json!({ "kind": "approval", "requestId": rid, "toolName": norm.0, "input": norm.1, "status": "pending" }),
            true,
        );
        self.turn.set_status(SessionStatus::Waiting);
    }

    fn on_line(&self, line: &str) {
        if line.trim().is_empty() {
            return;
        }
        let Ok(m) = serde_json::from_str::<Value>(line) else {
            self.turn.raw(line);
            return;
        };
        let is_chunk =
            m["method"] == "session/update" && m["params"]["update"]["sessionUpdate"].as_str().is_some_and(|s| s.ends_with("_chunk"));
        if !is_chunk && !self.state.lock().expect("acp state").replaying {
            self.turn.raw(line);
        }
        let has_id = m.get("id").is_some();
        if has_id && m.get("method").is_none() {
            let id = m["id"].as_i64().unwrap_or(-1);
            if let Some(tx) = self.calls.lock().expect("acp calls").remove(&id) {
                let _ = tx.send(match m.get("error").filter(|e| !e.is_null()) {
                    Some(e) => Err(e.clone()),
                    None => Ok(m.get("result").cloned().unwrap_or(Value::Null)),
                });
            }
            return;
        }
        match m["method"].as_str() {
            Some("session/update") => self.on_update(&m["params"]["update"]),
            Some("session/request_permission") => {
                self.on_permission(m["id"].as_i64().unwrap_or(-1), m.get("params").cloned().unwrap_or_else(|| json!({})))
            }
            _ if has_id => {
                self.send(json!({ "jsonrpc": "2.0", "id": m["id"], "error": { "code": -32601, "message": "Method not found" } }))
            }
            _ => {}
        }
    }

    async fn run(&self) -> Result<(), Value> {
        let init = self
            .call("initialize", json!({ "protocolVersion": 1, "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false } }))
            .await?;
        let caps = &init["agentCapabilities"];
        // images go to the model only if the agent says it takes them; otherwise just the path
        let images_supported = truthy(caps["promptCapabilities"].get("image"));
        if !self.turn.attachments.is_empty() {
            self.turn.raw(
                &json!({ "hub": "promptCapabilities", "value": caps.get("promptCapabilities").cloned().unwrap_or(Value::Null) })
                    .to_string(),
            );
        }
        let extra_dirs = caps["sessionCapabilities"]["additionalDirectories"].as_bool().unwrap_or(false)
            || truthy(caps["sessionCapabilities"].get("additionalDirectories"));
        let meta = self.spec.session_meta.map(|f| f(self.mode));
        let session_params = |sid: Option<&str>| {
            let mut p = Map::new();
            if let Some(s) = sid {
                p.insert("sessionId".into(), json!(s));
            }
            p.insert("cwd".into(), json!(self.turn.cwd));
            let servers: Vec<Value> = if self.spec.mcp_via_param {
                self.turn
                    .mcp_servers
                    .iter()
                    .map(|m| {
                        let env: Vec<Value> = m.env.iter().map(|(k, v)| json!({ "name": k, "value": v })).collect();
                        json!({ "name": m.name, "command": m.command, "args": m.args, "env": env })
                    })
                    .collect()
            } else {
                vec![]
            };
            p.insert("mcpServers".into(), json!(servers));
            if extra_dirs && !self.turn.extra_dirs.is_empty() {
                p.insert("additionalDirectories".into(), json!(self.turn.extra_dirs));
            }
            if let Some(m) = &meta {
                p.insert("_meta".into(), m.clone());
            }
            Value::Object(p)
        };
        let existing = self.state.lock().expect("acp state").session_id.clone();
        let mut is_new = existing.is_none();
        let mut preface = String::new();
        let r: Value;
        if let Some(sid) = existing.as_deref().filter(|_| truthy(caps.get("loadSession"))) {
            self.state.lock().expect("acp state").replaying = true;
            let loaded = self.call("session/load", session_params(Some(sid))).await;
            self.state.lock().expect("acp state").replaying = false;
            r = loaded?;
        } else {
            if existing.is_some() {
                // the agent can't load sessions: continue in a new one with a recap of
                // everything before the message being sent now (the last user event)
                let hist = self.turn.history();
                let cut = hist.iter().rposition(|e| e["kind"] == "user");
                let prev = transcript(match cut {
                    Some(c) => &hist[..c],
                    None => &hist[..],
                });
                if !prev.is_empty() {
                    preface = format!("Earlier conversation in this session (for context):\n\n{prev}\n\n---\n\n");
                }
                is_new = true;
            }
            r = self.call("session/new", session_params(None)).await?;
            let sid = r["sessionId"].as_str().map(String::from).unwrap_or_else(|| r["sessionId"].to_string());
            self.state.lock().expect("acp state").session_id = Some(sid.clone());
            self.turn.set_agent_session_id(&sid);
        }
        let r = if r.is_null() { json!({}) } else { r };
        let sid = self.state.lock().expect("acp state").session_id.clone().unwrap_or_default();
        remember_models(&self.turn, self.spec.kind, &r);
        {
            let mut st = self.state.lock().expect("acp state");
            if st.model.is_none() {
                st.model = current_model(&r);
            }
        }
        if let Some(value) = self.turn.session().agent.model
            && self.call("session/set_config_option", json!({ "sessionId": sid, "configId": "model", "value": value })).await.is_err()
        {
            self.call("session/set_model", json!({ "sessionId": sid, "modelId": value })).await?;
        }
        if let Some((_, mode_id)) = self.spec.mode_ids.iter().find(|(m, _)| *m == self.mode)
            && r["modes"]["availableModes"].as_array().is_some_and(|ms| ms.iter().any(|m| m["id"] == *mode_id))
        {
            let _ = self.call("session/set_mode", json!({ "sessionId": sid, "modeId": mode_id })).await;
        }
        if self.spec.mode_config_option {
            let value = if self.mode == PermissionMode::Plan { "plan" } else { "build" };
            let _ = self.call("session/set_config_option", json!({ "sessionId": sid, "configId": "mode", "value": value })).await;
        }
        // system context (manifest) goes into the first message of a new session
        let head = match (&self.turn.system_prompt, is_new) {
            (Some(p), true) => format!("{p}\n\n---\n\n"),
            _ => String::new(),
        };
        let started = Instant::now();
        let text = format!("{head}{preface}{}", self.turn.prompt);
        let mut prompt = vec![json!({ "type": "text", "text": text })];
        if images_supported {
            for a in self.turn.attachments.iter().filter(|a| crate::attachments::is_model_image(a)) {
                if let Ok(b) = std::fs::read(&a.path) {
                    prompt.push(json!({ "type": "image", "mimeType": a.mime, "data": super::claude::base64_encode(&b) }));
                }
            }
        }
        let res = self.call("session/prompt", json!({ "sessionId": sid, "prompt": prompt })).await?;
        let mut st = self.state.lock().expect("acp state");
        self.flush_thought(&mut st);
        self.flush_text(&mut st);
        st.finished = true;
        let stop = res["stopReason"].as_str().unwrap_or_default();
        let body = obj(vec![
            ("kind", Some(json!("result"))),
            ("ok", Some(json!(stop == "end_turn" || stop == "max_tokens"))),
            ("stopped", (stop == "cancelled").then(|| json!(true))),
            ("durationMs", Some(json!(started.elapsed().as_millis() as u64))),
            ("model", st.model.clone().map(Value::String)),
        ]);
        drop(st);
        self.turn.put(&format!("res:{}", self.run_tag), body, true);
        Ok(())
    }

    /// End of the turn: expire pending requests, stop the agent.
    fn end(&self) {
        let pending = {
            let mut st = self.state.lock().expect("acp state");
            st.settled = true;
            std::mem::take(&mut st.pending)
        };
        for (id, _) in pending {
            self.turn.patch(&format!("ap:{id}:{}", self.run_tag), json!({ "status": "expired" }), true);
        }
        if let Some(pid) = self.pid {
            kill_group(pid, libc::SIGTERM);
        }
    }
}

struct Handle(Arc<Client>);

impl RunHandle for Handle {
    fn stop(&self, reason: StopReason) {
        let c = &self.0;
        let sid = {
            let mut st = c.state.lock().expect("acp state");
            st.stopped = true;
            st.stop_reason = Some(reason);
            st.session_id.clone()
        };
        if let Some(sid) = sid {
            c.send(json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": sid } }));
        }
        if let Some(pid) = c.pid {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(1500)).await;
                kill_group(pid, libc::SIGTERM);
            });
        }
    }

    fn respond(&self, request_id: &str, d: ApprovalDecision) -> bool {
        let c = &self.0;
        let id: i64 = request_id.split(':').next().and_then(|s| s.parse().ok()).unwrap_or(-1);
        let (params, none_left) = {
            let mut st = c.state.lock().expect("acp state");
            let Some(pos) = st.pending.iter().position(|(i, _)| *i == id) else { return false };
            let (_, p) = st.pending.remove(pos);
            (p, st.pending.is_empty())
        };
        c.answer(id, &params, d.decision);
        let status = if d.decision == Decision::Allow { "allowed" } else { "denied" };
        let patch = obj(vec![("status", Some(json!(status))), ("reason", d.reason.map(Value::String))]);
        c.turn.patch(&format!("ap:{request_id}"), patch, true);
        if none_left {
            c.turn.set_status(SessionStatus::Running);
        }
        true
    }
}

pub fn start(spec: &'static Spec, turn: Arc<Turn>) -> Result<Started, String> {
    let session = turn.session();
    let mode = session.agent.permission_mode.unwrap_or(PermissionMode::Ask);
    let (base_cmd, base_args, base_env) = (spec.command)(&turn, mode);
    let (cmd, args) = turn.wrap.apply(&base_cmd, base_args);
    let mut child = tokio::process::Command::new(&cmd)
        .args(&args)
        .current_dir(&turn.cwd)
        .env_clear()
        .envs(child_env())
        .envs(turn.env.iter().cloned())
        .envs(base_env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("Failed to start {} ({cmd}): {e}", spec.label))?;
    let pid = child.id();
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        while let Some(line) = out_rx.recv().await {
            if stdin.write_all(format!("{line}\n").as_bytes()).await.is_err() {
                break;
            }
            let _ = stdin.flush().await;
        }
    });
    let client = Arc::new(Client {
        spec,
        turn: turn.clone(),
        state: Mutex::new(State { session_id: session.agent_session_id.clone(), model: session.agent.model.clone(), ..Default::default() }),
        out: out_tx,
        calls: Arc::default(),
        next_id: AtomicI64::new(0),
        run_tag: base36(jiff::Timestamp::now().as_millisecond() as u64),
        mode,
        pid,
    });
    let reader = {
        let c = client.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                c.on_line(&line);
            }
        })
    };
    let tail = Arc::new(Mutex::new(std::collections::VecDeque::<String>::new()));
    let err_reader = {
        let (t, tail) = (turn.clone(), tail.clone());
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                t.raw(&format!("[stderr] {}", line.trim_end()));
                let mut q = tail.lock().expect("stderr tail");
                q.push_back(line);
                if q.len() > 50 {
                    q.pop_front();
                }
            }
        })
    };
    let done = {
        let c = client.clone();
        tokio::spawn(async move {
            let closed = async {
                let _ = reader.await;
                let _ = err_reader.await;
                child.wait().await.ok()
            };
            tokio::select! {
                result = c.run() => {
                    if let Err(err) = result {
                        let mut st = c.state.lock().expect("acp state");
                        c.flush_text(&mut st);
                        if !st.stopped {
                            let msg = err["message"].as_str().map(String::from).unwrap_or_else(|| err.to_string());
                            let text = match &st.last_provider_error { Some(p) => format!("{msg}\n{p}"), None => msg };
                            drop(st);
                            c.turn.put(&format!("err:{}", c.run_tag), json!({ "kind": "notice", "level": "error", "text": text }), true);
                        }
                    }
                    let (stopped, reason) = { let st = c.state.lock().expect("acp state"); (st.stopped, st.stop_reason) };
                    if stopped {
                        c.turn.put(&format!("stop:{}", c.run_tag), json!({ "kind": "notice", "level": "info", "text": stop_notice(reason.unwrap_or(StopReason::User)) }), true);
                    }
                    c.end();
                }
                status = closed => {
                    let (finished, stopped, settled) = { let st = c.state.lock().expect("acp state"); (st.finished, st.stopped, st.settled) };
                    // settled: the hub stopped the process; not an agent crash
                    if !finished && !stopped && !settled {
                        let code = status.and_then(|s| s.code()).map(|c| c.to_string()).unwrap_or_else(|| "null".into());
                        let joined = tail.lock().expect("stderr tail").iter().cloned().collect::<Vec<_>>().join("\n");
                        let all: Vec<&str> = joined.trim().split('\n').collect();
                        let t = all[all.len().saturating_sub(15)..].join("\n");
                        let text = if t.is_empty() { format!("{} exited with code {code}", c.spec.label) } else { format!("{} exited with code {code}:\n{t}", c.spec.label) };
                        c.turn.put(&format!("err:exit:{}", c.run_tag), json!({ "kind": "notice", "level": "error", "text": text }), true);
                    }
                    c.end();
                }
            }
        })
    };
    Ok((Arc::new(Handle(client)), done))
}

// ---------- agent specs ----------

fn opencode_bin() -> String {
    super::agent_bin("HUBNESS_OPENCODE_BIN", "opencode")
}

fn grok_bin() -> String {
    super::agent_bin("HUBNESS_GROK_BIN", "grok")
}

/// Codex ACP adapter command (`HUBNESS_CODEX_ACP` overrides the global `codex-acp`).
fn codex_command() -> Vec<String> {
    match std::env::var("HUBNESS_CODEX_ACP")
        .ok()
        .map(|v| v.split_whitespace().map(String::from).collect::<Vec<_>>())
        .filter(|v| !v.is_empty())
    {
        Some(v) => v,
        None => vec![super::resolve("codex-acp")],
    }
}

/// Codex via the @zed-industries/codex-acp adapter; modes via session/set_mode when declared.
pub static CODEX: Spec = Spec {
    kind: "codex",
    label: "Codex",
    command: |_, _| {
        let mut c = codex_command();
        let cmd = c.remove(0);
        (cmd, c, vec![])
    },
    mcp_via_param: true,
    session_meta: None,
    mode_ids: &[
        (PermissionMode::Ask, "read-only"),
        (PermissionMode::Plan, "read-only"),
        (PermissionMode::AcceptEdits, "auto"),
        (PermissionMode::Bypass, "full-access"),
    ],
    native_permissions: false,
    describe_info: None,
    mode_config_option: false,
};

fn opencode_permissions(mode: PermissionMode) -> Value {
    let (edit, bash, web) = match mode {
        PermissionMode::Bypass => ("allow", "allow", "allow"),
        PermissionMode::AcceptEdits => ("allow", "ask", "ask"),
        PermissionMode::Plan => ("deny", "ask", "ask"),
        PermissionMode::Ask => ("ask", "ask", "ask"),
    };
    json!({ "edit": edit, "bash": bash, "webfetch": web })
}

/// OpenCode reports retries after a provider error in `_meta['opencode/retry']`; the real
/// cause (for OpenRouter) is in error.metadata.raw of the response body.
pub fn describe_opencode_retry(u: &Value) -> Option<(String, String)> {
    let r = u["_meta"].get("opencode/retry")?;
    let e = r.get("error").filter(|e| truthy(Some(e)))?;
    let mut reason = e["message"].as_str().unwrap_or("Provider error").to_string();
    if let Some(body) = e["response"]["body"].as_str().and_then(|b| serde_json::from_str::<Value>(b).ok())
        && let Some(raw) = body["error"]["metadata"]["raw"].as_str().or_else(|| body["error"]["message"].as_str())
    {
        reason = raw.to_string();
    }
    let status = e
        .get("status")
        .filter(|s| truthy(Some(s)))
        .map(|s| format!("{} ", s.as_str().map(String::from).unwrap_or_else(|| s.to_string())))
        .unwrap_or_default();
    let when = r["nextRetryAt"].as_str().and_then(|t| t.parse::<jiff::Timestamp>().ok()).map(|t| {
        let secs = (t.as_millisecond() - jiff::Timestamp::now().as_millisecond()) as f64 / 1000.0;
        secs.round().max(0.0) as i64
    });
    let error = format!("{status}{reason}").trim().to_string();
    let attempt = r["attempt"].as_i64().map(|a| a.to_string()).unwrap_or_else(|| r["attempt"].to_string());
    let retry = format!("Provider error {error} — retry #{attempt}{}", when.map(|w| format!(" in {w}s")).unwrap_or_default());
    Some((retry, error))
}

/// OpenCode via `opencode acp`; permission mode and MCP servers via inline config (it
/// ignores the ACP mcpServers parameter).
pub static OPENCODE: Spec = Spec {
    kind: "opencode",
    label: "Opencode",
    command: |turn, mode| {
        let mcp: Map<String, Value> = turn
            .mcp_servers
            .iter()
            .map(|m| {
                let mut command = vec![m.command.clone()];
                command.extend(m.args.iter().cloned());
                let env: Map<String, Value> = m.env.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
                (m.name.clone(), json!({ "type": "local", "command": command, "environment": env, "enabled": true }))
            })
            .collect();
        let config = json!({ "permission": opencode_permissions(mode), "mcp": mcp });
        (opencode_bin(), vec!["acp".into()], vec![("OPENCODE_CONFIG_CONTENT".into(), config.to_string())])
    },
    mcp_via_param: false,
    session_meta: None,
    mode_ids: &[],
    native_permissions: true,
    describe_info: Some(describe_opencode_retry),
    mode_config_option: true,
};

/// Grok CLI via `grok agent stdio`; yolo mode only under bypass.
pub static GROK: Spec = Spec {
    kind: "grok",
    label: "Grok CLI",
    command: |_, _| (grok_bin(), vec!["agent".into(), "stdio".into()], vec![]),
    mcp_via_param: true,
    session_meta: Some(|mode| json!({ "yoloMode": mode == PermissionMode::Bypass })),
    mode_ids: &[],
    native_permissions: false,
    describe_info: None,
    mode_config_option: false,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_normalize_like_the_ts_client() {
        assert_eq!(normalize_tool("bash", &json!({ "command": ["ls", "-la"] })), ("Bash".into(), json!({ "command": "ls -la" })));
        assert_eq!(
            normalize_tool("anything", &json!({ "filePath": "/a", "oldString": "x", "newString": "y" })),
            ("Edit".into(), json!({ "file_path": "/a", "old_string": "x", "new_string": "y" }))
        );
        assert_eq!(normalize_tool("", &json!({ "pattern": "x", "path": "src" })).0, "Grep");
        assert_eq!(normalize_tool("", &json!({ "pattern": "*.rs" })).0, "Glob");
        assert_eq!(
            normalize_tool("x", &json!({ "variant": "UseTool", "tool_name": "read", "tool_input": { "file_path": "/f" } })).0,
            "Read"
        );
        assert_eq!(normalize_tool("hub__open_pane", &json!({ "kind": "url" })).0, "mcp__hub__open_pane");
        assert_eq!(normalize_tool(&"x".repeat(41), &json!({})).0, "Tool");
    }

    #[test]
    fn client_side_policy() {
        assert_eq!(auto_decision(PermissionMode::Bypass, None), Some(Decision::Allow));
        assert_eq!(auto_decision(PermissionMode::AcceptEdits, Some("edit")), Some(Decision::Allow));
        assert_eq!(auto_decision(PermissionMode::Plan, Some("delete")), Some(Decision::Deny));
        assert_eq!(auto_decision(PermissionMode::Ask, Some("edit")), None);
        assert_eq!(auto_decision(PermissionMode::AcceptEdits, Some("execute")), None);
    }
}
