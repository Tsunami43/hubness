//! hubness MCP server — a port of `mcp/hub-mcp.ts` (stdio, line-delimited JSON-RPC 2.0),
//! run as `hubness mcp`. The agent gets:
//! - `open_pane` / `close_pane`: show a site preview, a file, or a diff next to the chat;
//! - `preview_*`: its own headless browser (on the hub) to open the app it builds, look at
//!   screenshots, click, type, run JavaScript and read the console — to check its work.
//!
//! The hub attaches it to every agent session.
//!
//! Env: `HUB_URL` (http://127.0.0.1:8787), `HUB_SESSION_ID`.

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// An MCP server attached to an agent session.
#[derive(Debug, Clone)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// The hub's own MCP server for a session: this binary with the `mcp` subcommand.
pub fn hub_mcp(self_url: &str, session_id: &str) -> Option<McpServer> {
    let exe = std::env::current_exe().ok()?;
    Some(McpServer {
        name: "hub".into(),
        command: exe.to_string_lossy().into_owned(),
        args: vec!["mcp".into()],
        env: {
            let mut env = vec![("HUB_URL".into(), self_url.to_string()), ("HUB_SESSION_ID".into(), session_id.to_string())];
            // the MCP server calls the hub back with the per-process service token
            if let Some(t) = crate::auth::service_token() {
                env.push(("HUB_TOKEN".into(), t));
            }
            env
        },
    })
}

fn tools() -> Value {
    json!([
        {
            "name": "open_pane",
            "description": "Show something to the user in a panel next to the chat in the hubness UI, instead of describing it in text. Use it to show a running web app or dev server (kind=url, e.g. http://localhost:5173), a dashboard (Grafana), a file you created or changed (kind=file, path relative to the working directory), or the git diff of a file (kind=diff). The panel replaces the previous one.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "enum": ["url", "file", "diff"], "description": "What to show" },
                    "target": { "type": "string", "description": "URL for kind=url; file path for kind=file or kind=diff" },
                    "title": { "type": "string", "description": "Short title for the panel header" }
                },
                "required": ["kind", "target"]
            }
        },
        {
            "name": "close_pane",
            "description": "Close the panel next to the chat in the hubness UI.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "preview_open",
            "description": "Open a page in your own headless browser on the hub to check a web app you are building or running (e.g. http://localhost:5173/settings — localhost works, also for apps in the project container). Returns the title, console errors and failed requests, and a screenshot unless screenshot=false. Use it after changing UI code to see the result yourself instead of asking the user. width/height set the window (e.g. 390×844 to check the phone layout).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "Address to open" },
                    "width": { "type": "number", "description": "Window width in CSS pixels (default 1280; under 600 emulates a phone)" },
                    "height": { "type": "number", "description": "Window height (default 800)" },
                    "screenshot": { "type": "boolean", "description": "Attach a screenshot (default true)" }
                },
                "required": ["url"]
            }
        },
        {
            "name": "preview_screenshot",
            "description": "Screenshot of the page open in your preview browser.",
            "inputSchema": { "type": "object", "properties": { "fullPage": { "type": "boolean", "description": "The whole scrollable page, not only the window" } } }
        },
        {
            "name": "preview_click",
            "description": "Click an element in the preview browser, found by CSS selector or by its visible text (buttons, links, labels).",
            "inputSchema": { "type": "object", "properties": { "selector": { "type": "string" }, "text": { "type": "string", "description": "Visible text of the element" }, "screenshot": shot_prop() } }
        },
        {
            "name": "preview_type",
            "description": "Type into a field in the preview browser (replaces its value). Find it by CSS selector or by text (label / placeholder).",
            "inputSchema": {
                "type": "object",
                "properties": { "selector": { "type": "string" }, "text": { "type": "string", "description": "Label, placeholder or visible text" }, "value": { "type": "string", "description": "What to type" }, "screenshot": shot_prop() },
                "required": ["value"]
            }
        },
        {
            "name": "preview_press",
            "description": "Press a key in the preview browser: Enter, Tab, Escape, Backspace, ArrowDown… or one character.",
            "inputSchema": { "type": "object", "properties": { "key": { "type": "string" }, "screenshot": shot_prop() }, "required": ["key"] }
        },
        {
            "name": "preview_eval",
            "description": "Run a JavaScript expression in the preview page (awaited) and get its JSON value — read the DOM, app state, localStorage…",
            "inputSchema": { "type": "object", "properties": { "expression": { "type": "string" } }, "required": ["expression"] }
        },
        {
            "name": "preview_console",
            "description": "New console messages, uncaught errors and failed requests of the preview page since the last call — yours and those the user saw in the hubness Preview panel for the same app.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "preview_close",
            "description": "Close your preview browser page.",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ])
}

/// The `screenshot` property of the action tools.
fn shot_prop() -> Value {
    json!({ "type": "boolean", "description": "Attach a screenshot of the page after the action" })
}

/// One `preview_*` tool: a POST /api/sessions/:id/browser action, formatted for the agent.
async fn browser(hub_url: &str, session_id: &str, args: &Value, action: &str) -> Value {
    if session_id.is_empty() {
        return text("hubness session is not set", true);
    }
    let mut body = json!({ "action": action });
    for k in ["url", "width", "height", "selector", "text", "value", "key", "expression", "fullPage", "screenshot"] {
        if let Some(v) = args.get(k).filter(|v| !v.is_null()) {
            body[k] = v.clone();
        }
    }
    if action == "open" && body.get("screenshot").is_none() {
        body["screenshot"] = json!(true);
    }
    let (status, raw) = match post_hub(hub_url, &format!("/api/sessions/{session_id}/browser"), &body).await {
        Ok(r) => r,
        Err(e) => return text(&format!("Error: {e}"), true),
    };
    let r: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    if !(200..300).contains(&status) {
        let msg = r.get("error").and_then(Value::as_str).map(String::from).unwrap_or_else(|| format!("hub responded {status}"));
        return text(&msg, true);
    }
    let get = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("");
    let mut lines: Vec<String> = vec![];
    if !get("text").is_empty() {
        lines.push(get("text").into());
    }
    if !get("url").is_empty() {
        let title = if get("title").is_empty() { String::new() } else { format!("{} — ", get("title")) };
        lines.push(format!("Page: {title}{}", get("url")));
    }
    match r.get("logs").and_then(Value::as_array).filter(|l| !l.is_empty()) {
        Some(logs) => {
            lines.push(String::new());
            lines.push("Console / errors:".into());
            for l in logs {
                let src = if l.get("source").and_then(Value::as_str) == Some("user") { "user " } else { "" };
                let level = l.get("level").and_then(Value::as_str).unwrap_or("");
                lines.push(format!("[{src}{level}] {}", l.get("text").and_then(Value::as_str).unwrap_or("")));
            }
        }
        None if action != "close" => lines.push("No console errors.".into()),
        None => {}
    }
    let mut content = vec![json!({ "type": "text", "text": lines.join("\n") })];
    if let Some(shot) = r.get("screenshot").and_then(Value::as_str) {
        content.push(json!({ "type": "image", "data": shot, "mimeType": "image/png" }));
    }
    json!({ "content": content, "isError": false })
}

fn text(t: &str, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": t }], "isError": is_error })
}

/// POST JSON to the hub (plain HTTP/1.1 to the local hub; no TLS needed): (status, body).
async fn post_hub(hub_url: &str, path: &str, body: &Value) -> Result<(u16, String), String> {
    let authority = hub_url.strip_prefix("http://").ok_or("HUB_URL must be http://")?.trim_end_matches('/');
    let mut stream = tokio::net::TcpStream::connect(authority).await.map_err(|e| e.to_string())?;
    let body = body.to_string();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{auth}Connection: close\r\n\r\n{body}",
        body.len(),
        auth =
            std::env::var("HUB_TOKEN").ok().filter(|t| !t.is_empty()).map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default()
    );
    stream.write_all(req.as_bytes()).await.map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.map_err(|e| e.to_string())?;
    // split on bytes: the body may be cut anywhere, also inside a character
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n");
    let (head, rest) = match split {
        Some(at) => (String::from_utf8_lossy(&raw[..at]).into_owned(), &raw[at + 4..]),
        None => (String::from_utf8_lossy(&raw).into_owned(), &raw[raw.len()..]),
    };
    let status: u16 = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let chunked =
        head.lines().any(|l| l.to_ascii_lowercase().starts_with("transfer-encoding:") && l.to_ascii_lowercase().contains("chunked"));
    let body = if chunked { dechunk(rest) } else { rest.to_vec() };
    Ok((status, String::from_utf8_lossy(&body).into_owned()))
}

/// The body of a chunked response (sizes count bytes).
fn dechunk(mut rest: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(at) = rest.windows(2).position(|w| w == b"\r\n") {
        let size = String::from_utf8_lossy(&rest[..at]);
        let Ok(n) = usize::from_str_radix(size.split(';').next().unwrap_or("").trim(), 16) else { break };
        let tail = &rest[at + 2..];
        if n == 0 || tail.len() < n {
            break;
        }
        out.extend_from_slice(&tail[..n]);
        rest = tail[n..].strip_prefix(b"\r\n").unwrap_or(&tail[n..]);
    }
    out
}

/// POST the pane to the hub.
async fn call_hub(hub_url: &str, session_id: &str, pane: Value) -> Result<(), String> {
    if session_id.is_empty() {
        return Err("hubness session is not set".into());
    }
    let (status, detail) = post_hub(hub_url, &format!("/api/sessions/{session_id}/pane"), &json!({ "pane": pane })).await?;
    if !(200..300).contains(&status) {
        return Err(format!("hub responded {status}: {detail}"));
    }
    Ok(())
}

async fn handle(hub_url: &str, session_id: &str, m: &Value) -> Value {
    let id = m["id"].clone();
    let reply = |result: Value| json!({ "jsonrpc": "2.0", "id": id, "result": result });
    match m["method"].as_str().unwrap_or_default() {
        "initialize" => reply(json!({
            "protocolVersion": m["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "hubness", "version": env!("CARGO_PKG_VERSION") }
        })),
        "ping" => reply(json!({})),
        "tools/list" => reply(json!({ "tools": tools() })),
        "tools/call" => {
            let name = m["params"]["name"].as_str().unwrap_or_default();
            let args = &m["params"]["arguments"];
            let result = match name {
                "open_pane" => {
                    let kind = args["kind"].as_str().filter(|k| ["url", "file", "diff"].contains(k));
                    let target = match &args["target"] {
                        Value::String(s) if !s.is_empty() => Some(s.clone()),
                        Value::Null | Value::String(_) => None,
                        other => Some(other.to_string()),
                    };
                    match (kind, target) {
                        (Some(kind), Some(target)) => {
                            let mut pane = json!({ "kind": kind, "target": target });
                            if let Some(t) = args.get("title").filter(|t| !t.is_null() && *t != "") {
                                pane["title"] = json!(t.as_str().map(String::from).unwrap_or_else(|| t.to_string()));
                            }
                            match call_hub(hub_url, session_id, pane).await {
                                Ok(()) => text(&format!("Opened {kind} panel: {target}"), false),
                                Err(e) => text(&format!("Error: {e}"), true),
                            }
                        }
                        _ => text("kind and target are required", true),
                    }
                }
                "close_pane" => match call_hub(hub_url, session_id, Value::Null).await {
                    Ok(()) => text("Panel closed", false),
                    Err(e) => text(&format!("Error: {e}"), true),
                },
                other if other.starts_with("preview_") => browser(hub_url, session_id, args, &other["preview_".len()..]).await,
                other => text(&format!("Unknown tool {other}"), true),
            };
            reply(result)
        }
        other => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("Method not found: {other}") } }),
    }
}

/// Serve MCP on stdin/stdout until stdin closes.
pub async fn run_stdio() -> anyhow::Result<()> {
    let hub_url = std::env::var("HUB_URL").unwrap_or_else(|_| "http://127.0.0.1:8787".into());
    let session_id = std::env::var("HUB_SESSION_ID").unwrap_or_default();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let out = std::sync::Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));
    // every request is its own task: a browser action that takes its time (a slow page) does
    // not hold back a ping or the next call
    let mut calls = tokio::task::JoinSet::new();
    while let Some(line) = lines.next_line().await? {
        while calls.try_join_next().is_some() {}
        let Ok(m) = serde_json::from_str::<Value>(&line) else { continue };
        if m.get("id").is_none() {
            continue; // notifications (initialized, etc.)
        }
        let (hub_url, session_id, out) = (hub_url.clone(), session_id.clone(), out.clone());
        calls.spawn(async move {
            let resp = handle(&hub_url, &session_id, &m).await;
            let mut out = out.lock().await;
            let _ = out.write_all(format!("{resp}\n").as_bytes()).await;
            let _ = out.flush().await;
        });
    }
    // stdin closed: the answers still on their way are written first
    while calls.join_next().await.is_some() {}
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunked_bodies_are_put_together_by_bytes() {
        // "жук" is 6 bytes; the chunk border falls inside a character
        let raw = "жук — ok".as_bytes();
        let mut wire = Vec::new();
        for part in [&raw[..3], &raw[3..]] {
            wire.extend_from_slice(format!("{:x};ext=1\r\n", part.len()).as_bytes());
            wire.extend_from_slice(part);
            wire.extend_from_slice(b"\r\n");
        }
        wire.extend_from_slice(b"0\r\n\r\n");
        assert_eq!(dechunk(&wire), raw);
        assert_eq!(dechunk(b"zz\r\nabc"), b"", "not a size: nothing");
        assert_eq!(dechunk(b"ff\r\nshort"), b"", "a chunk cut off is dropped");
        assert_eq!(dechunk(b""), b"");
    }
}
