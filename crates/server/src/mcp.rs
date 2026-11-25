//! hubness MCP server — a port of `mcp/hub-mcp.ts` (stdio, line-delimited JSON-RPC 2.0),
//! run as `hubness mcp`. The agent gets the `open_pane` tool: show a site preview, a file,
//! or a diff next to the chat; and `close_pane`. The hub attaches it to every agent session.
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
        env: vec![("HUB_URL".into(), self_url.to_string()), ("HUB_SESSION_ID".into(), session_id.to_string())],
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
        }
    ])
}

fn text(t: &str, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": t }], "isError": is_error })
}

/// POST the pane to the hub (plain HTTP/1.1 to the local hub; no TLS needed).
async fn call_hub(hub_url: &str, session_id: &str, pane: Value) -> Result<(), String> {
    if session_id.is_empty() {
        return Err("hubness session is not set".into());
    }
    let authority = hub_url.strip_prefix("http://").ok_or("HUB_URL must be http://")?.trim_end_matches('/');
    let mut stream = tokio::net::TcpStream::connect(authority).await.map_err(|e| e.to_string())?;
    let body = json!({ "pane": pane }).to_string();
    let req = format!(
        "POST /api/sessions/{session_id}/pane HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).await.map_err(|e| e.to_string())?;
    let mut resp = String::new();
    stream.read_to_string(&mut resp).await.map_err(|e| e.to_string())?;
    let status: u16 = resp.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    if !(200..300).contains(&status) {
        let detail = resp.split("\r\n\r\n").nth(1).unwrap_or_default();
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
    let mut out = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        let Ok(m) = serde_json::from_str::<Value>(&line) else { continue };
        if m.get("id").is_none() {
            continue; // notifications (initialized, etc.)
        }
        let resp = handle(&hub_url, &session_id, &m).await;
        out.write_all(format!("{resp}\n").as_bytes()).await?;
        out.flush().await?;
    }
    Ok(())
}
