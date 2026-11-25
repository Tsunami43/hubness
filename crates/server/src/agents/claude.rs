//! Claude Code CLI adapter — a port of `agents/claude/adapter.ts`.
//!
//! Protocol: `claude -p` with stream-json for input and output; permissions go through
//! `control_request`/`can_use_tool` on stdout and `control_response` on stdin
//! (`--permission-prompt-tool stdio`). Resume — `--resume <session_id>`.
//!

use std::collections::{HashMap, HashSet, VecDeque};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hubness_core::{PermissionMode, SessionStatus};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use super::{ApprovalDecision, Decision, RunHandle, Started, StopReason, kill_group, stop_notice};
use crate::config::child_env;
use crate::runtime::Turn;

const OUTPUT_LIMIT: usize = 20_000;

/// Claude Code CLI binary.
pub fn claude_bin() -> String {
    super::agent_bin("HUBNESS_CLAUDE_BIN", "claude")
}

/// A legacy CLAUDE_CONFIG_DIR profile (kept only so old sessions resume where their transcript lives).
#[derive(Deserialize)]
struct Profile {
    id: String,
    dir: Option<String>,
}

/// (profile id, CLAUDE_CONFIG_DIR) of a session's account; unknown → the default profile.
fn profile(turn: &Turn, account: Option<&str>) -> (String, Option<String>) {
    match account.filter(|a| *a != "default") {
        None => ("default".into(), None),
        Some(id) => turn
            .kv_get::<Vec<Profile>>("claude.profiles")
            .unwrap_or_default()
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| (p.id, p.dir))
            .unwrap_or(("default".into(), None)),
    }
}

fn build_args(turn: &Turn) -> Vec<String> {
    let s = turn.session();
    let a = &s.agent;
    let mut args: Vec<String> = [
        "-p",
        "--verbose",
        "--output-format=stream-json",
        "--input-format=stream-json",
        "--include-partial-messages",
        "--permission-prompt-tool",
        "stdio",
    ]
    .map(String::from)
    .to_vec();
    if let Some(m) = &a.model {
        args.extend(["--model".into(), m.clone()]);
    }
    match a.permission_mode.unwrap_or(PermissionMode::Ask) {
        PermissionMode::Bypass => args.push("--dangerously-skip-permissions".into()),
        PermissionMode::AcceptEdits => args.extend(["--permission-mode".into(), "acceptEdits".into()]),
        PermissionMode::Plan => args.extend(["--permission-mode".into(), "plan".into()]),
        PermissionMode::Ask => args.extend(["--permission-mode".into(), "default".into()]),
    }
    for d in &turn.extra_dirs {
        args.extend(["--add-dir".into(), d.clone()]);
    }
    if let Some(p) = &turn.system_prompt {
        args.extend(["--append-system-prompt".into(), p.clone()]);
    }
    if !turn.mcp_servers.is_empty() {
        let servers: serde_json::Map<String, Value> = turn
            .mcp_servers
            .iter()
            .map(|m| {
                let env: serde_json::Map<String, Value> = m.env.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
                (m.name.clone(), json!({ "command": m.command, "args": m.args, "env": env }))
            })
            .collect();
        args.extend(["--mcp-config".into(), json!({ "mcpServers": servers }).to_string()]);
        // hub tools are safe (they only show a pane), so don't ask for permission
        args.push("--allowedTools".into());
        for m in &turn.mcp_servers {
            args.extend([format!("mcp__{}__open_pane", m.name), format!("mcp__{}__close_pane", m.name)]);
        }
    }
    if let Some(id) = &s.agent_session_id {
        args.extend(["--resume".into(), id.clone()]);
    }
    args
}

/// Like JavaScript's `s.slice(0, n)` on UTF-16 units.
fn limit_output(s: String) -> String {
    if s.encode_utf16().count() <= OUTPUT_LIMIT {
        return s;
    }
    format!("{}\n… (truncated)", crate::sessions::truncate(&s, OUTPUT_LIMIT))
}

fn stringify_tool_output(content: Option<&Value>) -> String {
    let s = match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|c| match c["type"].as_str() {
                Some("text") => c["text"].as_str().unwrap_or_default().to_string(),
                Some("image") => "[image]".into(),
                _ => c.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::Null) | None => "\"\"".into(),
        Some(other) => other.to_string(),
    };
    limit_output(s)
}

pub(crate) fn base64_encode(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// JavaScript template-literal rendering of an id (`${v}`).
fn id_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "undefined".into(),
        other => other.to_string(),
    }
}

struct Block {
    uid: String,
    text_kind: bool,
    text: String,
}

#[derive(Default)]
struct State {
    model: Option<String>,
    /// 'none' means subscription OAuth login.
    api_key_source: Option<String>,
    limit: Option<Value>,
    got_result: bool,
    stopped: bool,
    stop_reason: Option<StopReason>,
    streamed: HashSet<String>,
    denied: HashSet<String>,
    /// control_requests awaiting a response, in arrival order.
    pending: Vec<(String, Value)>,
    cur_msg: String,
    blocks: HashMap<i64, Block>,
}

struct Handle {
    turn: Arc<Turn>,
    state: Arc<Mutex<State>>,
    stdin: mpsc::UnboundedSender<Option<String>>,
    pid: Option<u32>,
    exited: Arc<AtomicBool>,
}

fn send(tx: &mpsc::UnboundedSender<Option<String>>, v: Value) {
    let _ = tx.send(Some(v.to_string()));
}

impl RunHandle for Handle {
    fn stop(&self, reason: StopReason) {
        {
            let mut st = self.state.lock().expect("claude state");
            st.stopped = true;
            st.stop_reason = Some(reason);
        }
        let Some(pid) = self.pid else { return };
        kill_group(pid, libc::SIGTERM);
        let exited = self.exited.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            if !exited.load(Ordering::SeqCst) {
                kill_group(pid, libc::SIGKILL);
            }
        });
    }

    fn respond(&self, request_id: &str, d: ApprovalDecision) -> bool {
        let (req, none_left) = {
            let mut st = self.state.lock().expect("claude state");
            let Some(pos) = st.pending.iter().position(|(id, _)| id == request_id) else { return false };
            let (_, req) = st.pending.remove(pos);
            if d.decision == Decision::Deny
                && let Some(t) = req["tool_use_id"].as_str()
            {
                st.denied.insert(t.to_string());
            }
            (req, st.pending.is_empty())
        };
        let response = match d.decision {
            Decision::Allow => {
                let mut input = req["input"].clone();
                if req["tool_name"] == "AskUserQuestion"
                    && let (Some(answers), Value::Object(obj)) = (&d.answers, &mut input)
                {
                    obj.insert("answers".into(), Value::Object(answers.clone()));
                }
                let mut r = json!({ "behavior": "allow", "updatedInput": input });
                // plan approval: switch the CLI session out of plan mode, as the CLI itself suggests
                if req["tool_name"] == "ExitPlanMode" && req["permission_suggestions"].is_array() {
                    r["updatedPermissions"] = req["permission_suggestions"].clone();
                }
                r
            }
            Decision::Deny => {
                let reason = d.reason.as_deref().map(str::trim).filter(|r| !r.is_empty()).unwrap_or("The user denied this action");
                json!({ "behavior": "deny", "message": reason })
            }
        };
        send(
            &self.stdin,
            json!({ "type": "control_response", "response": { "subtype": "success", "request_id": request_id, "response": response } }),
        );
        let mut patch = Map::new();
        patch.insert("status".into(), json!(if d.decision == Decision::Allow { "allowed" } else { "denied" }));
        if let Some(a) = d.answers {
            patch.insert("answers".into(), Value::Object(a));
        }
        if let Some(r) = d.reason {
            patch.insert("reason".into(), json!(r));
        }
        self.turn.patch(&format!("ap:{request_id}"), Value::Object(patch), true);
        if none_left {
            self.turn.set_status(SessionStatus::Running);
        }
        true
    }
}

pub fn start(turn: Arc<Turn>) -> Result<Started, String> {
    let bin = claude_bin();
    let session = turn.session();
    let (profile_id, config_dir) = profile(&turn, session.agent.account.as_deref());
    let (cmd, args) = turn.wrap.apply(&bin, build_args(&turn));
    let mut command = tokio::process::Command::new(&cmd);
    command
        .args(&args)
        .current_dir(&turn.cwd)
        .env_clear()
        .envs(child_env())
        .envs(turn.env.iter().cloned())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // own process group: stop() kills the whole tree
        .process_group(0);
    if let Some(dir) = &config_dir {
        command.env("CLAUDE_CONFIG_DIR", dir);
    }
    let mut child = command.spawn().map_err(|e| format!("Failed to start Claude Code ({bin}): {e}"))?;
    let pid = child.id();
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    // stdin writer: lines until None (end of input). A CLI that dies right away closes
    // stdin: write errors are ignored, the exit handler reports the failed turn
    let (tx, mut rx) = mpsc::unbounded_channel::<Option<String>>();
    tokio::spawn(async move {
        while let Some(Some(line)) = rx.recv().await {
            if stdin.write_all(format!("{line}\n").as_bytes()).await.is_err() {
                break;
            }
            let _ = stdin.flush().await;
        }
    });

    let state = Arc::new(Mutex::new(State::default()));
    let exited = Arc::new(AtomicBool::new(false));

    let reader = {
        let (turn, state, tx) = (turn.clone(), state.clone(), tx.clone());
        let profile_id = profile_id.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                on_line(&turn, &state, &tx, &profile_id, &line);
            }
        })
    };
    let tail = Arc::new(Mutex::new(VecDeque::<String>::new()));
    let err_reader = {
        let (turn, tail) = (turn.clone(), tail.clone());
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                turn.raw(&format!("[stderr] {}", line.trim_end()));
                let mut t = tail.lock().expect("stderr tail");
                t.push_back(line);
                if t.len() > 50 {
                    t.pop_front();
                }
            }
        })
    };

    let done = {
        let (turn, state, exited) = (turn.clone(), state.clone(), exited.clone());
        tokio::spawn(async move {
            let _ = reader.await;
            let _ = err_reader.await;
            let status = child.wait().await;
            exited.store(true, Ordering::SeqCst);
            let (pending, stopped, reason, got_result) = {
                let mut st = state.lock().expect("claude state");
                (std::mem::take(&mut st.pending), st.stopped, st.stop_reason, st.got_result)
            };
            for (id, _) in pending {
                turn.patch(&format!("ap:{id}"), json!({ "status": "expired" }), true);
            }
            if stopped {
                let text = stop_notice(reason.unwrap_or(StopReason::User));
                turn.put(&format!("stop:{}", now_ms()), json!({ "kind": "notice", "level": "info", "text": text }), true);
            } else if !got_result {
                let code = status.ok().and_then(|s| s.code()).map(|c| c.to_string()).unwrap_or_else(|| "null".into());
                let lines: Vec<String> = tail.lock().expect("stderr tail").iter().cloned().collect();
                let joined = lines.join("\n");
                let all: Vec<&str> = joined.trim().split('\n').collect();
                let tail = all[all.len().saturating_sub(15)..].join("\n");
                let text = if tail.is_empty() {
                    format!("Claude Code exited with code {code}")
                } else {
                    format!("Claude Code exited with code {code}:\n{tail}")
                };
                turn.put(&format!("err:exit:{}", now_ms()), json!({ "kind": "notice", "level": "error", "text": text }), true);
            }
        })
    };

    // first user message; images go to the model as content blocks (paths are in the text)
    let images: Vec<Value> = turn
        .attachments
        .iter()
        .filter(|a| crate::attachments::is_model_image(a))
        .filter_map(|a| std::fs::read(&a.path).ok().map(|b| (a, b)))
        .map(|(a, b)| json!({ "type": "image", "source": { "type": "base64", "media_type": a.mime, "data": base64_encode(&b) } }))
        .collect();
    let content = if images.is_empty() {
        json!(turn.prompt)
    } else {
        let mut blocks = vec![json!({ "type": "text", "text": turn.prompt })];
        blocks.extend(images);
        Value::Array(blocks)
    };
    send(&tx, json!({ "type": "user", "message": { "role": "user", "content": content } }));
    let handle = Handle { turn, state, stdin: tx, pid, exited };
    Ok((Arc::new(handle), done))
}

fn on_line(turn: &Turn, state: &Mutex<State>, tx: &mpsc::UnboundedSender<Option<String>>, profile_id: &str, line: &str) {
    if line.trim().is_empty() {
        return;
    }
    let Ok(j) = serde_json::from_str::<Value>(line) else {
        turn.raw(line);
        return;
    };
    if j["type"] != "stream_event" {
        turn.raw(line);
    }
    // don't show subagent (Task) events, only the top level
    if j.get("parent_tool_use_id").is_some_and(|v| !v.is_null() && v != "") {
        return;
    }
    match j["type"].as_str().unwrap_or_default() {
        "system" if j["subtype"] == "init" => {
            if let Some(id) = j["session_id"].as_str() {
                turn.set_agent_session_id(id);
            }
            let mut st = state.lock().expect("claude state");
            st.model = j["model"].as_str().map(String::from);
            st.api_key_source = j["apiKeySource"].as_str().map(String::from);
        }
        "rate_limit_event" => on_rate_limit(turn, state, profile_id, &j["rate_limit_info"]),
        "stream_event" => on_stream_event(turn, state, &j["event"]),
        "assistant" => {
            if let Some(m) = j["message"]["model"].as_str() {
                state.lock().expect("claude state").model = Some(m.to_string());
            }
            on_assistant(turn, state, &j);
        }
        "user" => on_user(turn, state, &j),
        "control_request" => on_control_request(turn, state, tx, &j),
        "result" => {
            state.lock().expect("claude state").got_result = true;
            if let Some(id) = j["session_id"].as_str() {
                turn.set_agent_session_id(id);
            }
            let key = j.get("uuid").filter(|v| !v.is_null()).map(id_str).unwrap_or_else(|| now_ms().to_string());
            let is_error = j["is_error"].as_bool().unwrap_or(false);
            if is_error && let Some(text) = j["result"].as_str() {
                turn.put(&format!("err:{key}"), json!({ "kind": "notice", "level": "error", "text": text }), true);
            }
            let st = state.lock().expect("claude state");
            let mut res = Map::new();
            res.insert("kind".into(), json!("result"));
            res.insert("ok".into(), json!(!is_error));
            if let Some(d) = j.get("duration_ms").filter(|v| !v.is_null()) {
                res.insert("durationMs".into(), d.clone());
            }
            res.insert("billing".into(), json!(if st.api_key_source.as_deref() == Some("none") { "subscription" } else { "api" }));
            if let Some(c) = j.get("total_cost_usd").filter(|v| !v.is_null()) {
                res.insert("costUsd".into(), c.clone());
            }
            if let Some(l) = &st.limit {
                res.insert("limit".into(), l.clone());
            }
            if let Some(m) = &st.model {
                res.insert("model".into(), json!(m));
            }
            drop(st);
            turn.put(&format!("res:{key}"), Value::Object(res), true);
            let _ = tx.send(None);
        }
        _ => {}
    }
}

/// Subscription limits: remembered for the Harness page, and the most utilized window goes
/// into the turn result.
fn on_rate_limit(turn: &Turn, state: &Mutex<State>, profile_id: &str, info: &Value) {
    let Some(windows) = info["unifiedWindows"].as_object() else { return };
    let limits: Vec<Value> = windows
        .iter()
        .filter_map(|(id, w)| {
            let u = w["utilization"].as_f64()?;
            let resets = w.get("resetsAt").filter(|v| !v.is_null() && *v != &json!(0) && *v != &json!("")).and_then(|v| {
                let secs = v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))?;
                jiff::Timestamp::from_millisecond((secs * 1000.0) as i64).ok().map(crate::db::iso)
            });
            Some(json!({ "id": id, "utilization": (if u <= 1.0 { u * 100.0 } else { u }).round() as i64, "resetsAt": resets }))
        })
        .collect();
    if !limits.is_empty() {
        let key = if profile_id == "default" { "claude.limits".to_string() } else { format!("claude.limits:{profile_id}") };
        turn.kv_set(&key, &Value::Array(limits));
    }
    // stable sort, most utilized first (as the TS sort)
    let mut ws: Vec<(&String, &Value)> = windows.iter().collect();
    ws.sort_by(|a, b| {
        let (ua, ub) = (a.1["utilization"].as_f64().unwrap_or(0.0), b.1["utilization"].as_f64().unwrap_or(0.0));
        ub.partial_cmp(&ua).unwrap_or(std::cmp::Ordering::Equal)
    });
    if let Some((id, w)) = ws.first()
        && w["utilization"].is_number()
    {
        state.lock().expect("claude state").limit = Some(json!({ "window": id, "utilization": w["utilization"] }));
    }
}

fn on_stream_event(turn: &Turn, state: &Mutex<State>, e: &Value) {
    let index = e["index"].as_i64().unwrap_or_default();
    match e["type"].as_str().unwrap_or_default() {
        "message_start" => {
            let mut st = state.lock().expect("claude state");
            st.cur_msg = e["message"]["id"].as_str().map(String::from).unwrap_or_else(|| format!("m{}", now_ms()));
            st.blocks.clear();
        }
        "content_block_start" => {
            let t = e["content_block"]["type"].as_str().unwrap_or_default();
            if t != "text" && t != "thinking" {
                return;
            }
            let uid = {
                let mut st = state.lock().expect("claude state");
                let uid = format!("{}:{}:{index}", if t == "text" { "t" } else { "th" }, st.cur_msg);
                st.blocks.insert(index, Block { uid: uid.clone(), text_kind: t == "text", text: String::new() });
                let cur = st.cur_msg.clone();
                st.streamed.insert(cur);
                uid
            };
            if t == "text" {
                turn.put(&uid, json!({ "kind": "text", "text": "", "streaming": true }), false);
            }
        }
        "content_block_delta" => {
            let update = {
                let mut st = state.lock().expect("claude state");
                let Some(b) = st.blocks.get_mut(&index) else { return };
                match e["delta"]["type"].as_str() {
                    Some("text_delta") => {
                        b.text.push_str(e["delta"]["text"].as_str().unwrap_or_default());
                        Some((b.uid.clone(), b.text.clone()))
                    }
                    Some("thinking_delta") => {
                        b.text.push_str(e["delta"]["thinking"].as_str().unwrap_or_default());
                        None
                    }
                    _ => None,
                }
            };
            if let Some((uid, text)) = update {
                turn.patch(&uid, json!({ "text": text }), false);
            }
        }
        "content_block_stop" => {
            let Some(b) = state.lock().expect("claude state").blocks.remove(&index) else { return };
            if b.text_kind {
                turn.put(&b.uid, json!({ "kind": "text", "text": b.text, "streaming": false }), true);
            } else if !b.text.trim().is_empty() {
                turn.put(&b.uid, json!({ "kind": "thinking", "text": b.text }), true);
            }
        }
        _ => {}
    }
}

fn on_assistant(turn: &Turn, state: &Mutex<State>, j: &Value) {
    let msg = &j["message"];
    let msg_id = id_str(&msg["id"]);
    let streamed = state.lock().expect("claude state").streamed.contains(&msg_id);
    let Some(blocks) = msg["content"].as_array() else { return };
    for (i, b) in blocks.iter().enumerate() {
        match b["type"].as_str().unwrap_or_default() {
            "tool_use" => {
                let id = id_str(&b["id"]);
                let uid = format!("tool:{id}");
                if turn.get(&uid).is_none() {
                    let mut body = Map::new();
                    body.insert("kind".into(), json!("tool"));
                    body.insert("toolId".into(), b["id"].clone());
                    body.insert("name".into(), b["name"].clone());
                    if let Some(input) = b.get("input") {
                        body.insert("input".into(), input.clone());
                    }
                    body.insert("status".into(), json!("running"));
                    turn.put(&uid, Value::Object(body), true);
                }
            }
            "text" if !streamed => {
                if let Some(text) = b["text"].as_str().filter(|t| !t.is_empty()) {
                    turn.put(&format!("t:{msg_id}:a{i}"), json!({ "kind": "text", "text": text }), true);
                }
            }
            "thinking" if !streamed => {
                if let Some(text) = b["thinking"].as_str().filter(|t| !t.trim().is_empty()) {
                    turn.put(&format!("th:{msg_id}:a{i}"), json!({ "kind": "thinking", "text": text }), true);
                }
            }
            _ => {}
        }
    }
}

fn on_user(turn: &Turn, state: &Mutex<State>, j: &Value) {
    let Some(content) = j["message"]["content"].as_array() else { return };
    for c in content.iter().filter(|c| c["type"] == "tool_result") {
        let tool_id = id_str(&c["tool_use_id"]);
        let uid = format!("tool:{tool_id}");
        if turn.get(&uid).is_none() {
            continue;
        }
        let status = if c["is_error"].as_bool().unwrap_or(false) {
            if state.lock().expect("claude state").denied.contains(&tool_id) { "denied" } else { "error" }
        } else {
            "ok"
        };
        turn.patch(&uid, json!({ "status": status, "output": stringify_tool_output(c.get("content")) }), true);
    }
}

fn on_control_request(turn: &Turn, state: &Mutex<State>, tx: &mpsc::UnboundedSender<Option<String>>, j: &Value) {
    let req = j.get("request").cloned().unwrap_or_else(|| json!({}));
    let request_id = id_str(&j["request_id"]);
    if req["subtype"] != "can_use_tool" {
        let error = format!("unsupported: {}", id_str(&req["subtype"]));
        send(tx, json!({ "type": "control_response", "response": { "subtype": "error", "request_id": j["request_id"], "error": error } }));
        return;
    }
    let mut body = Map::new();
    body.insert("kind".into(), json!("approval"));
    body.insert("requestId".into(), j["request_id"].clone());
    body.insert("toolName".into(), req["tool_name"].clone());
    if let Some(input) = req.get("input") {
        body.insert("input".into(), input.clone());
    }
    body.insert("status".into(), json!("pending"));
    if req["tool_name"] == "AskUserQuestion" && req["input"]["questions"].is_array() {
        body.insert("questions".into(), req["input"]["questions"].clone());
    }
    state.lock().expect("claude state").pending.push((request_id.clone(), req));
    turn.put(&format!("ap:{request_id}"), Value::Object(body), true);
    turn.set_status(SessionStatus::Waiting);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_output_like_the_ts_adapter() {
        assert_eq!(stringify_tool_output(Some(&json!("a"))), "a");
        assert_eq!(
            stringify_tool_output(Some(&json!([{ "type": "text", "text": "x" }, { "type": "image" }, { "k": 1 }]))),
            "x\n[image]\n{\"k\":1}"
        );
        assert_eq!(stringify_tool_output(None), "\"\"");
        let long = "é".repeat(OUTPUT_LIMIT + 5);
        assert!(stringify_tool_output(Some(&json!(long))).ends_with("\n… (truncated)"));
    }
}
