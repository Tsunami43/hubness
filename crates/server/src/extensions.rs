//! What each agent is extended with (the Harness page, as `mcp-servers.ts`, `extensions.ts`
//! and the agents' `extensions.ts` modules in TS): MCP servers in the agents' own config
//! files — the matrix "server × agent", adding, connecting, disconnecting and deleting them
//! in the user configs — and the skills, rules files, subagents, commands and plugins the
//! agents read from their folders. Config edits keep everything else in the file, are atomic,
//! keep a one-time `<file>.hubness.bak` and never touch a file that doesn't parse.

use std::io::Read;
use std::path::{Path, PathBuf};

use axum::Router;
use axum::extract::{Path as UrlPath, Query, State};
use axum::routing::{delete, get, put};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use toml_edit::{DocumentMut, Item, Table, value};

use crate::AppState;
use crate::db::projects;
use crate::error::{ApiResult, HttpError};
use crate::harness::LABELS;
use crate::http::JsonBody;

const CLAUDE: &str = "claude-code";
const CODEX: &str = "codex";
const OPENCODE: &str = "opencode";
const GROK: &str = "grok";

/// The hub's own server: attached to every hub session by the hub itself.
const BUILTIN_MCP: &str = "hub";

// ---------- where the agents keep their config ----------

fn env_dir(k: &str) -> Option<PathBuf> {
    std::env::var(k).ok().filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn home() -> PathBuf {
    env_dir("HOME").unwrap_or_else(|| PathBuf::from("/"))
}

fn claude_dir() -> PathBuf {
    env_dir("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home().join(".claude"))
}

fn claude_json() -> PathBuf {
    match env_dir("CLAUDE_CONFIG_DIR") {
        Some(d) => d.join(".claude.json"),
        None => home().join(".claude.json"),
    }
}

fn codex_home() -> PathBuf {
    env_dir("CODEX_HOME").unwrap_or_else(|| home().join(".codex"))
}

fn opencode_dir() -> PathBuf {
    env_dir("XDG_CONFIG_HOME").unwrap_or_else(|| home().join(".config")).join("opencode")
}

fn grok_home() -> PathBuf {
    env_dir("GROK_HOME").unwrap_or_else(|| home().join(".grok"))
}

/// opencode.json, or opencode.jsonc when only that one exists.
fn opencode_file() -> PathBuf {
    let (json, jsonc) = (opencode_dir().join("opencode.json"), opencode_dir().join("opencode.jsonc"));
    if !json.exists() && jsonc.exists() { jsonc } else { json }
}

fn p(path: &Path) -> String {
    crate::paths::to_string(path)
}

// ---------- config files ----------

const MAX_CONFIG: u64 = 16 * 1024 * 1024;

fn unreadable(file: &Path, what: &str) -> HttpError {
    HttpError::new(409, format!("{} is not valid {what} — fix it or edit it by hand; the hub left it untouched", p(file)))
}

fn read_text(file: &Path) -> ApiResult<Option<String>> {
    if !file.exists() {
        return Ok(None);
    }
    if std::fs::metadata(file)?.len() > MAX_CONFIG {
        return Err(HttpError::new(409, format!("{} is too large to edit", p(file))));
    }
    Ok(Some(std::fs::read_to_string(file).map_err(|_| unreadable(file, "UTF-8 text"))?))
}

/// Atomic write through the real path (a symlink stays a symlink), keeping the file mode;
/// with `backup`, the first write keeps a copy of the original as `<file>.hubness.bak`.
fn write_config(file: &Path, text: &str, backup: bool) -> ApiResult<()> {
    use std::os::unix::fs::PermissionsExt;
    let exists = file.exists();
    let real = if exists { std::fs::canonicalize(file)? } else { file.to_path_buf() };
    if let Some(parent) = real.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut mode = 0o644;
    if exists {
        mode = std::fs::metadata(&real)?.permissions().mode() & 0o777;
        let bak = PathBuf::from(format!("{}.hubness.bak", p(file)));
        if backup && !bak.exists() {
            std::fs::copy(&real, &bak)?;
        }
    }
    let tmp = PathBuf::from(format!("{}.hubness-{}.tmp", p(&real), std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    std::fs::rename(&tmp, &real)?;
    Ok(())
}

fn read_json_file(file: &Path) -> ApiResult<Option<Map<String, Value>>> {
    let Some(text) = read_text(file)? else { return Ok(None) };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(m)) => Ok(Some(m)),
        _ => Err(unreadable(file, "JSON")),
    }
}

fn write_json_file(file: &Path, cfg: &Map<String, Value>) -> ApiResult<()> {
    write_config(file, &(serde_json::to_string_pretty(cfg).expect("json") + "\n"), true)
}

fn read_toml_file(file: &Path) -> ApiResult<Option<DocumentMut>> {
    let Some(text) = read_text(file)? else { return Ok(None) };
    text.parse::<DocumentMut>().map(Some).map_err(|_| unreadable(file, "TOML"))
}

// ---------- MCP definitions ----------

/// A server with its secrets (env / header values): never sent to clients as is.
#[derive(Clone, Debug, PartialEq)]
struct McpDef {
    name: String,
    transport: &'static str,
    command: Option<String>,
    args: Vec<String>,
    env: Vec<(String, String)>,
    url: Option<String>,
    headers: Vec<(String, String)>,
    enabled: bool,
}

fn str_of(v: &Value) -> Option<String> {
    v.as_str().map(String::from)
}

fn str_list(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

fn str_map(v: &Value) -> Vec<(String, String)> {
    v.as_object().map(|m| m.iter().filter_map(|(k, x)| x.as_str().map(|s| (k.clone(), s.to_string()))).collect()).unwrap_or_default()
}

fn map_value(m: &[(String, String)]) -> Value {
    Value::Object(m.iter().map(|(k, v)| (k.clone(), json!(v))).collect())
}

/// Claude Code: `{type, command, args, env}` / `{type, url, headers}`.
fn claude_def(name: &str, v: &Value) -> McpDef {
    let url = str_of(&v["url"]);
    let transport = match v["type"].as_str() {
        Some("sse") => "sse",
        Some("http") => "http",
        _ if url.is_some() && v.get("command").is_none_or(|c| c.is_null() || c == "") => "http",
        _ => "stdio",
    };
    McpDef {
        name: name.into(),
        transport,
        command: str_of(&v["command"]),
        args: str_list(&v["args"]),
        env: str_map(&v["env"]),
        url,
        headers: str_map(&v["headers"]),
        enabled: true,
    }
}

fn claude_value(d: &McpDef) -> Value {
    if d.transport == "stdio" {
        return json!({ "type": "stdio", "command": d.command, "args": d.args, "env": map_value(&d.env) });
    }
    let mut m = Map::new();
    m.insert("type".into(), json!(d.transport));
    m.insert("url".into(), json!(d.url));
    if !d.headers.is_empty() {
        m.insert("headers".into(), map_value(&d.headers));
    }
    Value::Object(m)
}

/// OpenCode: `{type: local, command: [...], environment}` / `{type: remote, url, headers}`.
fn opencode_def(name: &str, v: &Value) -> McpDef {
    let enabled = v["enabled"] != false;
    if v["type"] == "remote" || (v["url"].is_string() && v["type"] != "local") {
        return McpDef {
            name: name.into(),
            transport: "http",
            command: None,
            args: vec![],
            env: vec![],
            url: str_of(&v["url"]),
            headers: str_map(&v["headers"]),
            enabled,
        };
    }
    let cmd = str_list(&v["command"]);
    McpDef {
        name: name.into(),
        transport: "stdio",
        command: cmd.first().cloned(),
        args: cmd.iter().skip(1).cloned().collect(),
        env: str_map(&v["environment"]),
        url: None,
        headers: vec![],
        enabled,
    }
}

fn opencode_value(d: &McpDef) -> Value {
    let mut m = Map::new();
    if d.transport == "stdio" {
        m.insert("type".into(), json!("local"));
        let mut cmd = vec![d.command.clone().unwrap_or_default()];
        cmd.extend(d.args.iter().cloned());
        m.insert("command".into(), json!(cmd));
        if !d.env.is_empty() {
            m.insert("environment".into(), map_value(&d.env));
        }
    } else {
        // OpenCode finds out itself whether a remote server speaks streamable HTTP or SSE
        m.insert("type".into(), json!("remote"));
        m.insert("url".into(), json!(d.url));
        if !d.headers.is_empty() {
            m.insert("headers".into(), map_value(&d.headers));
        }
    }
    m.insert("enabled".into(), json!(true));
    Value::Object(m)
}

fn json_defs(servers: Option<&Value>, kind: &str) -> Vec<McpDef> {
    let Some(m) = servers.and_then(Value::as_object) else { return vec![] };
    m.iter()
        .filter(|(_, v)| v.is_object())
        .map(|(name, v)| if kind == CLAUDE { claude_def(name, v) } else { opencode_def(name, v) })
        .collect()
}

/// The header map of a TOML server: Codex `http_headers`, Grok `headers`.
fn headers_key(kind: &str) -> &'static str {
    if kind == CODEX { "http_headers" } else { "headers" }
}

fn toml_str(item: Option<&Item>) -> Option<String> {
    item.and_then(|i| i.as_str()).map(String::from)
}

fn toml_list(item: Option<&Item>) -> Vec<String> {
    item.and_then(|i| i.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
}

fn toml_map(item: Option<&Item>) -> Vec<(String, String)> {
    item.and_then(|i| i.as_table_like())
        .map(|t| t.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.to_string(), s.to_string()))).collect())
        .unwrap_or_default()
}

fn toml_defs(doc: &DocumentMut, kind: &str) -> Vec<McpDef> {
    let Some(servers) = doc.get("mcp_servers").and_then(Item::as_table_like) else { return vec![] };
    let disabled = toml_list(doc.get("disabled_mcp_servers"));
    servers
        .iter()
        .filter_map(|(name, item)| {
            let t = item.as_table_like()?;
            let url = toml_str(t.get("url"));
            let transport = if toml_str(t.get("type")).as_deref() == Some("sse") {
                "sse"
            } else if url.is_some() && t.get("command").is_none() {
                "http"
            } else {
                "stdio"
            };
            Some(McpDef {
                name: name.to_string(),
                transport,
                command: toml_str(t.get("command")),
                args: toml_list(t.get("args")),
                env: toml_map(t.get("env")),
                url,
                headers: toml_map(t.get(headers_key(kind))),
                enabled: t.get("enabled").and_then(Item::as_bool) != Some(false) && !disabled.iter().any(|d| d == name),
            })
        })
        .collect()
}

fn toml_table(d: &McpDef, kind: &str) -> Table {
    let inline = |m: &[(String, String)]| {
        let mut t = toml_edit::InlineTable::new();
        for (k, v) in m {
            t.insert(k, v.as_str().into());
        }
        value(t)
    };
    let mut t = Table::new();
    if d.transport == "stdio" {
        t["command"] = value(d.command.clone().unwrap_or_default());
        t["args"] = value(d.args.iter().map(String::as_str).collect::<toml_edit::Array>());
        if !d.env.is_empty() {
            t["env"] = inline(&d.env);
        }
    } else {
        if d.transport == "sse" {
            t["type"] = value("sse");
        }
        t["url"] = value(d.url.clone().unwrap_or_default());
        if !d.headers.is_empty() {
            t[headers_key(kind)] = inline(&d.headers);
        }
    }
    t
}

fn cant_edit(file: &Path) -> HttpError {
    HttpError::new(409, format!("{}: the hub can't change this server without touching the rest of the file — edit it by hand", p(file)))
}

// ---------- one agent's MCP store ----------

fn is_toml(kind: &str) -> bool {
    kind == CODEX || kind == GROK
}

fn user_file(kind: &str) -> PathBuf {
    match kind {
        CLAUDE => claude_json(),
        CODEX => codex_home().join("config.toml"),
        OPENCODE => opencode_file(),
        _ => grok_home().join("config.toml"),
    }
}

fn transports(kind: &str) -> &'static [&'static str] {
    if kind == CODEX { &["stdio", "http"] } else { &["stdio", "http", "sse"] }
}

/// Servers in the user file; [] when it is missing; a 409 when it doesn't parse.
fn read_user(kind: &str, file: &Path) -> ApiResult<Vec<McpDef>> {
    if is_toml(kind) {
        return Ok(read_toml_file(file)?.map(|d| toml_defs(&d, kind)).unwrap_or_default());
    }
    let key = if kind == CLAUDE { "mcpServers" } else { "mcp" };
    Ok(read_json_file(file)?.map(|m| json_defs(m.get(key), kind)).unwrap_or_default())
}

/// Servers a project adds (shown, never written); unreadable files are skipped.
fn read_project(kind: &str, dir: &Path) -> Vec<(String, Vec<McpDef>)> {
    let mut out = vec![];
    match kind {
        CLAUDE => {
            if let Ok(Some(cfg)) = read_json_file(&dir.join(".mcp.json")) {
                out.push((p(&dir.join(".mcp.json")), json_defs(cfg.get("mcpServers"), kind)));
            }
            // the CLI's local scope: projects[<path>].mcpServers in .claude.json
            if let Ok(Some(cfg)) = read_json_file(&claude_json()) {
                let servers = cfg.get("projects").and_then(|ps| ps.get(p(dir))).and_then(|x| x.get("mcpServers"));
                out.push((p(&claude_json()), json_defs(servers, kind)));
            }
        }
        OPENCODE => {
            for name in ["opencode.json", "opencode.jsonc"] {
                if let Ok(Some(cfg)) = read_json_file(&dir.join(name)) {
                    out.push((p(&dir.join(name)), json_defs(cfg.get("mcp"), kind)));
                }
            }
        }
        _ => {
            let file = if kind == CODEX { dir.join(".codex").join("config.toml") } else { dir.join(".grok").join("config.toml") };
            if file != user_file(kind)
                && file.exists()
                && let Ok(Some(doc)) = read_toml_file(&file)
            {
                out.push((p(&file), toml_defs(&doc, kind)));
            }
        }
    }
    out
}

/// Add or replace a server in the user file.
fn put_def(kind: &str, file: &Path, def: &McpDef) -> ApiResult<()> {
    if is_toml(kind) {
        let mut doc = read_toml_file(file)?.unwrap_or_default();
        if doc.get("mcp_servers").is_none() {
            let mut t = Table::new();
            t.set_implicit(true);
            doc.insert("mcp_servers", Item::Table(t));
        }
        let servers = doc.get_mut("mcp_servers").and_then(Item::as_table_mut).ok_or_else(|| cant_edit(file))?;
        servers.remove(&def.name);
        servers.insert(&def.name, Item::Table(toml_table(def, kind)));
        return write_config(file, &doc.to_string(), true);
    }
    let (key, init, v) = if kind == CLAUDE {
        ("mcpServers", Map::new(), claude_value(def))
    } else {
        let mut init = Map::new();
        init.insert("$schema".into(), json!("https://opencode.ai/config.json"));
        ("mcp", init, opencode_value(def))
    };
    let mut cfg = read_json_file(file)?.unwrap_or(init);
    if !cfg.get(key).is_some_and(Value::is_object) {
        cfg.insert(key.into(), json!({}));
    }
    cfg[key].as_object_mut().expect("an object").insert(def.name.clone(), v);
    write_json_file(file, &cfg)
}

/// Remove a server from the user file; false when it wasn't there.
fn remove_def(kind: &str, file: &Path, name: &str) -> ApiResult<bool> {
    if is_toml(kind) {
        let Some(mut doc) = read_toml_file(file)? else { return Ok(false) };
        let Some(servers) = doc.get_mut("mcp_servers").and_then(Item::as_table_like_mut) else { return Ok(false) };
        if servers.remove(name).is_none() {
            return Ok(false);
        }
        write_config(file, &doc.to_string(), true)?;
        return Ok(true);
    }
    let key = if kind == CLAUDE { "mcpServers" } else { "mcp" };
    let Some(mut cfg) = read_json_file(file)? else { return Ok(false) };
    let Some(servers) = cfg.get_mut(key).and_then(Value::as_object_mut) else { return Ok(false) };
    if servers.shift_remove(name).is_none() {
        return Ok(false);
    }
    write_json_file(file, &cfg)?;
    Ok(true)
}

// ---------- the matrix ----------

struct Found {
    kind: &'static str,
    scope: &'static str,
    source: String,
    def: McpDef,
}

fn scan(project: Option<&Path>) -> (Vec<Found>, Vec<Value>) {
    let mut found = vec![];
    let mut agents = vec![];
    for (kind, label, _) in LABELS {
        let user = user_file(kind);
        let mut error = Value::Null;
        match read_user(kind, &user) {
            Ok(defs) => found.extend(defs.into_iter().map(|def| Found { kind, scope: "user", source: p(&user), def })),
            Err(e) => error = json!(e.message),
        }
        agents.push(json!({ "kind": kind, "label": label, "supported": true, "configPath": p(&user), "transports": transports(kind), "error": error }));
        if let Some(dir) = project {
            for (source, defs) in read_project(kind, dir) {
                found.extend(defs.into_iter().map(|def| Found { kind, scope: "project", source: source.clone(), def }));
            }
        }
    }
    (found, agents)
}

fn builtin_row() -> Value {
    let agents: Map<String, Value> = LABELS.iter().map(|(k, _, _)| (k.to_string(), json!("builtin"))).collect();
    json!({
        "name": BUILTIN_MCP, "transport": "stdio", "command": null, "args": [], "env": ["HUB_URL", "HUB_SESSION_ID"],
        "url": null, "headers": [], "builtIn": true, "agents": agents, "entries": [],
    })
}

fn matrix(found: &[Found], agents: Vec<Value>) -> Value {
    let mut names: Vec<&str> = found.iter().map(|f| f.def.name.as_str()).filter(|n| *n != BUILTIN_MCP).collect();
    names.sort();
    names.dedup();
    let mut rows = vec![builtin_row()];
    for name in names {
        let all: Vec<&Found> = found.iter().filter(|f| f.def.name == name).collect();
        // the definition shown: the first user-scope one (agents in registry order)
        let main = &all.iter().find(|f| f.scope == "user").unwrap_or(&all[0]).def;
        let mut entries = vec![];
        for (kind, _, _) in LABELS {
            for scope in ["user", "project"] {
                for f in all.iter().filter(|f| f.kind == *kind && f.scope == scope) {
                    entries.push(json!({ "agent": kind, "scope": scope, "source": f.source, "enabled": f.def.enabled }));
                }
            }
        }
        let per_agent: Map<String, Value> = LABELS
            .iter()
            .map(|(k, _, _)| (k.to_string(), entries.iter().find(|e| e["agent"] == *k).map(|e| e["scope"].clone()).unwrap_or(Value::Null)))
            .collect();
        rows.push(json!({
            "name": name,
            "transport": main.transport,
            "command": main.command,
            "args": main.args,
            "env": main.env.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            "url": main.url,
            "headers": main.headers.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            "builtIn": false,
            "agents": per_agent,
            "entries": entries,
        }));
    }
    json!({ "servers": rows, "agents": agents })
}

fn mcp_matrix(project: Option<&Path>) -> Value {
    let (found, agents) = scan(project);
    matrix(&found, agents)
}

// ---------- validation ----------

fn len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn name_ok(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn check_name(name: &str) -> ApiResult<()> {
    if !name_ok(name) {
        return Err(HttpError::bad_request("name: letters, digits, - and _ only (at most 64)"));
    }
    if name == BUILTIN_MCP {
        return Err(HttpError::bad_request(format!("“{BUILTIN_MCP}” is the hub's built-in server")));
    }
    Ok(())
}

fn env_name_ok(k: &str) -> bool {
    let mut cs = k.chars();
    k.len() <= 128 && cs.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn header_name_ok(k: &str) -> bool {
    (1..=128).contains(&k.len()) && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn check_map(m: &[(String, String)], ok: fn(&str) -> bool, what: &str) -> ApiResult<()> {
    if m.len() > 100 {
        return Err(HttpError::bad_request(format!("{what}: at most 100")));
    }
    for (k, v) in m {
        if !ok(k) {
            return Err(HttpError::bad_request(format!("{what}: bad name “{k}”")));
        }
        if len(v) > 4000 || v.contains(['\r', '\n', '\0']) {
            return Err(HttpError::bad_request(format!("{what}: bad value of {k}")));
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Transport {
    Stdio,
    Http,
    Sse,
}

#[derive(Deserialize)]
struct McpBody {
    name: String,
    transport: Transport,
    command: Option<String>,
    args: Option<Vec<String>>,
    env: Option<Map<String, Value>>,
    url: Option<String>,
    headers: Option<Map<String, Value>>,
    agents: Vec<String>,
}

fn string_map(m: Option<Map<String, Value>>, what: &str) -> ApiResult<Vec<(String, String)>> {
    m.unwrap_or_default()
        .into_iter()
        .map(|(k, v)| match v {
            Value::String(s) => Ok((k, s)),
            _ => Err(HttpError::bad_request(format!("{what}.{k}: Invalid input: expected string"))),
        })
        .collect()
}

/// A definition from the request, checked; values are kept as given.
fn def_of(b: McpBody) -> ApiResult<(McpDef, Vec<String>)> {
    if len(&b.name) > 64 {
        return Err(HttpError::bad_request("name: Too big: expected string to have <=64 characters"));
    }
    if b.agents.len() > 20 || b.agents.iter().any(|a| len(a) > 64) {
        return Err(HttpError::bad_request("agents: at most 20"));
    }
    check_name(&b.name)?;
    let env = string_map(b.env, "env")?;
    let headers = string_map(b.headers, "headers")?;
    let def = match b.transport {
        Transport::Stdio => {
            let command = b.command.as_deref().unwrap_or_default().trim().to_string();
            if command.is_empty() || len(&command) > 1000 || command.contains(['\r', '\n', '\0']) {
                return Err(HttpError::bad_request("command is required for a stdio server"));
            }
            let args = b.args.unwrap_or_default();
            if args.len() > 100 || args.iter().any(|a| len(a) > 2000 || a.contains('\0')) {
                return Err(HttpError::bad_request("args: at most 100, 2000 characters each"));
            }
            check_map(&env, env_name_ok, "env")?;
            McpDef { name: b.name, transport: "stdio", command: Some(command), args, env, url: None, headers: vec![], enabled: true }
        }
        t => {
            let url = b.url.as_deref().unwrap_or_default().trim().to_string();
            let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"));
            if !rest.is_some_and(|r| !r.is_empty() && !r.chars().any(char::is_whitespace)) || len(&url) > 2000 {
                return Err(HttpError::bad_request("url: an http(s) URL is required"));
            }
            check_map(&headers, header_name_ok, "headers")?;
            let transport = if matches!(t, Transport::Sse) { "sse" } else { "http" };
            McpDef { name: b.name, transport, command: None, args: vec![], env: vec![], url: Some(url), headers, enabled: true }
        }
    };
    Ok((def, b.agents))
}

/// The agent `kind` (404 unknown), checked to support `transport`.
fn supporting(kind: &str, transport: &str) -> ApiResult<&'static str> {
    let (k, label, _) = LABELS.iter().copied().find(|(k, _, _)| *k == kind).ok_or_else(|| HttpError::not_found("Unknown agent"))?;
    if !transports(k).contains(&transport) {
        return Err(HttpError::bad_request(format!("{label} does not support {} MCP servers", transport.to_uppercase())));
    }
    Ok(k)
}

fn agent_or_404(kind: &str) -> ApiResult<&'static str> {
    LABELS.iter().map(|(k, _, _)| *k).find(|k| *k == kind).ok_or_else(|| HttpError::not_found("Unknown agent"))
}

// ---------- changes (user configs only) ----------

/// Add a server to (or replace it in) each selected agent. All configs are checked before any
/// is written.
fn mcp_add(b: McpBody, project: Option<&Path>) -> ApiResult<Value> {
    let (def, kinds) = def_of(b)?;
    if kinds.is_empty() {
        return Err(HttpError::bad_request("agents: pick at least one agent"));
    }
    let mut targets: Vec<&str> = vec![];
    for k in &kinds {
        let k = supporting(k, def.transport)?;
        if !targets.contains(&k) {
            targets.push(k);
        }
    }
    for k in &targets {
        read_user(k, &user_file(k))?;
    }
    for k in &targets {
        put_def(k, &user_file(k), &def)?;
    }
    Ok(mcp_matrix(project))
}

/// Connect an existing server to one more agent: its definition is copied from another agent.
fn mcp_connect(name: &str, kind: &str, project: Option<&Path>) -> ApiResult<Value> {
    check_name(name)?;
    let (found, agents) = scan(project);
    let target = agent_or_404(kind)?;
    if found.iter().any(|f| f.kind == target && f.scope == "user" && f.def.name == name) {
        return Ok(matrix(&found, agents));
    }
    let from = found
        .iter()
        .find(|f| f.def.name == name && f.scope == "user")
        .or_else(|| found.iter().find(|f| f.def.name == name))
        .ok_or_else(|| HttpError::not_found("MCP server not found"))?;
    supporting(kind, from.def.transport)?;
    let file = user_file(target);
    read_user(target, &file)?;
    put_def(target, &file, &McpDef { enabled: true, ..from.def.clone() })?;
    Ok(mcp_matrix(project))
}

/// Remove a server from one agent's user config.
fn mcp_disconnect(name: &str, kind: &str, project: Option<&Path>) -> ApiResult<Value> {
    check_name(name)?;
    let agent = agent_or_404(kind)?;
    if !remove_def(agent, &user_file(agent), name)? {
        if let Some(dir) = project {
            let (found, _) = scan(Some(dir));
            if let Some(f) = found.iter().find(|f| f.kind == agent && f.def.name == name) {
                return Err(HttpError::new(409, format!("It comes from the project config {} — edit that file", f.source)));
            }
        }
        return Err(HttpError::not_found("This agent does not have the server"));
    }
    Ok(mcp_matrix(project))
}

/// Remove a server from every agent's user config (unreadable configs are left alone).
fn mcp_delete(name: &str, project: Option<&Path>) -> ApiResult<Value> {
    check_name(name)?;
    let mut removed = false;
    for (kind, _, _) in LABELS {
        if let Ok(true) = remove_def(kind, &user_file(kind), name) {
            removed = true;
        }
    }
    if !removed {
        return Err(HttpError::not_found("MCP server not found"));
    }
    Ok(mcp_matrix(project))
}

// ---------- extensions: skills, rules, subagents, commands, plugins ----------

const HEAD_BYTES: u64 = 64 * 1024;
const MAX_ENTRIES: usize = 500;
const MAX_RULES: u64 = 1024 * 1024;

/// The first `max` bytes of a file as text ('' when unreadable).
fn read_head(path: &Path, max: u64) -> String {
    let mut buf = vec![];
    match std::fs::File::open(path) {
        Ok(f) => {
            let _ = f.take(max).read_to_end(&mut buf);
            String::from_utf8_lossy(&buf).into_owned()
        }
        Err(_) => String::new(),
    }
}

fn cut(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn unquote(v: &str) -> String {
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        return serde_json::from_str::<String>(v).unwrap_or_else(|_| v[1..v.len() - 1].to_string());
    }
    if v.len() >= 2 && v.starts_with('\'') && v.ends_with('\'') {
        return v[1..v.len() - 1].replace("''", "'");
    }
    v.to_string()
}

/// YAML frontmatter, the small subset agents use: top-level `key: value` with plain or
/// quoted values, plain values continued on indented lines, and `>` / `|` block scalars.
/// Nested maps and lists are skipped. Never fails.
pub fn parse_frontmatter(text: &str) -> (Vec<(String, String)>, String) {
    use std::sync::OnceLock;
    static KEY: OnceLock<regex::Regex> = OnceLock::new();
    static BLOCK: OnceLock<regex::Regex> = OnceLock::new();
    let key_re = KEY.get_or_init(|| regex::Regex::new(r"^([A-Za-z0-9_-]+)\s*:(?:\s+(.*))?$").expect("regex"));
    let block_re = BLOCK.get_or_init(|| regex::Regex::new(r"^[>|][-+0-9]*$").expect("regex"));
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    if lines.first().map(|l| l.trim()) != Some("---") {
        return (vec![], text.to_string());
    }
    let Some(end) = lines.iter().enumerate().skip(1).find(|(_, l)| l.trim() == "---" || l.trim() == "...").map(|(i, _)| i) else {
        return (vec![], text.to_string());
    };
    let indented = |l: &str| l.starts_with(' ') || l.starts_with('\t');
    let mut fields: Vec<(String, String)> = vec![];
    let mut i = 1;
    while i < end {
        let caps = key_re.captures(lines[i]);
        i += 1;
        let Some(caps) = caps else { continue };
        let key = caps[1].to_string();
        let mut val = caps.get(2).map(|m| m.as_str()).unwrap_or_default().trim().to_string();
        let mut more: Vec<&str> = vec![];
        while i < end && (indented(lines[i]) || lines[i].trim().is_empty()) {
            more.push(lines[i]);
            i += 1;
        }
        while more.last().is_some_and(|l| l.trim().is_empty()) {
            more.pop();
        }
        if block_re.is_match(&val) {
            let parts: Vec<&str> = more.iter().map(|l| l.trim()).collect();
            val = if val.starts_with('>') {
                parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ")
            } else {
                parts.join("\n")
            };
        } else if val.starts_with('"') || val.starts_with('\'') {
            let mut parts = vec![val.as_str()];
            parts.extend(more.iter().map(|l| l.trim()));
            val = unquote(&parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" "));
        } else if !val.is_empty() {
            let mut parts = vec![val.as_str()];
            parts.extend(more.iter().map(|l| l.trim()));
            let mut joined = parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ");
            if let Some(h) = joined.find(" #") {
                joined.truncate(h);
            }
            val = joined;
        }
        let val = val.trim().to_string();
        match fields.iter_mut().find(|(k, _)| *k == key) {
            Some(f) => f.1 = val,
            None => fields.push((key, val)),
        }
    }
    (fields, lines[end + 1..].join("\n"))
}

fn field<'a>(fields: &'a [(String, String)], key: &str) -> &'a str {
    fields.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).unwrap_or_default()
}

/// The first meaningful line of a markdown body (a command without a description).
fn first_line(body: &str) -> String {
    for l in body.split('\n') {
        let s = l.trim_start_matches('#').trim();
        if !s.is_empty() {
            return cut(s, 200);
        }
    }
    String::new()
}

fn names(dir: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut out: Vec<String> = rd.filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| !n.starts_with('.')).collect();
    out.sort();
    out.truncate(MAX_ENTRIES);
    out
}

fn is_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file())
}

fn is_dir(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_dir())
}

/// A folder of extensions and its scope.
type Dir = (PathBuf, &'static str);

/// A rules file: label, path, scope, listed only when present.
struct Rules {
    label: &'static str,
    path: PathBuf,
    scope: &'static str,
    if_exists: bool,
}

#[derive(Default)]
struct Layout {
    skills: Vec<Dir>,
    rules: Vec<Rules>,
    agents: Vec<Dir>,
    commands: Vec<Dir>,
    /// From the agent's own config: agents listed in it, plugins and their skills.
    more_agents: Vec<Value>,
    plugins: Vec<Value>,
}

fn rules(label: &'static str, path: PathBuf, scope: &'static str) -> Rules {
    Rules { label, path, scope, if_exists: false }
}

fn layout(kind: &str, project: Option<&Path>) -> Layout {
    let mut l = Layout::default();
    match kind {
        CLAUDE => {
            let base = claude_dir();
            for (dirs, name) in [(&mut l.skills, "skills"), (&mut l.agents, "agents"), (&mut l.commands, "commands")] {
                dirs.push((base.join(name), "user"));
                if let Some(pr) = project {
                    dirs.push((pr.join(".claude").join(name), "project"));
                }
            }
            l.rules.push(rules("CLAUDE.md", base.join("CLAUDE.md"), "user"));
            if let Some(pr) = project {
                l.rules.push(rules("CLAUDE.md", pr.join("CLAUDE.md"), "project"));
                l.rules.push(rules("CLAUDE.local.md", pr.join("CLAUDE.local.md"), "project"));
            }
            // installed plugins (plugins/installed_plugins.json, v1 or v2) and their skills
            let enabled =
                crate::harness::read_json(&base.join("settings.json")).map(|s| s["enabledPlugins"].clone()).unwrap_or(Value::Null);
            if let Some(installed) = crate::harness::read_json(&base.join("plugins").join("installed_plugins.json"))
                .and_then(|j| j.get("plugins").and_then(Value::as_object).cloned())
            {
                for (id, v) in installed {
                    let rec = if v.is_array() { v[0].clone() } else { v };
                    let Some(install) = rec["installPath"].as_str() else { continue };
                    l.plugins.push(json!({
                        "name": id, "version": rec["version"].as_str(), "enabled": enabled[&id] == true, "path": install, "scope": "plugin",
                    }));
                    l.skills.push((PathBuf::from(install).join("skills"), "plugin"));
                }
            }
        }
        CODEX => {
            let base = codex_home();
            l.skills.push((base.join("skills"), "user"));
            if let Some(pr) = project {
                l.skills.push((pr.join(".codex").join("skills"), "project"));
                l.skills.push((pr.join(".agents").join("skills"), "project"));
            }
            l.rules.push(rules("AGENTS.md", base.join("AGENTS.md"), "user"));
            l.rules.push(Rules { label: "AGENTS.override.md", path: base.join("AGENTS.override.md"), scope: "user", if_exists: true });
            if let Some(pr) = project {
                l.rules.push(rules("AGENTS.md", pr.join("AGENTS.md"), "project"));
            }
            l.commands.push((base.join("prompts"), "user"));
        }
        OPENCODE => {
            let base = opencode_dir();
            for (dirs, name) in [(&mut l.skills, "skill"), (&mut l.agents, "agent"), (&mut l.commands, "command")] {
                dirs.push((base.join(name), "user"));
                dirs.push((base.join(format!("{name}s")), "user"));
                if let Some(pr) = project {
                    dirs.push((pr.join(".opencode").join(name), "project"));
                    dirs.push((pr.join(".opencode").join(format!("{name}s")), "project"));
                }
            }
            l.rules.push(rules("AGENTS.md", base.join("AGENTS.md"), "user"));
            if let Some(pr) = project {
                l.rules.push(rules("AGENTS.md", pr.join("AGENTS.md"), "project"));
            }
            // agents and plugins listed in the config itself
            let file = opencode_file();
            let cfg = crate::harness::read_json(&file).unwrap_or(Value::Null);
            if let Some(agents) = cfg["agent"].as_object() {
                for (name, a) in agents {
                    if !a.is_object() {
                        continue;
                    }
                    l.more_agents.push(json!({
                        "name": name,
                        "description": a["description"].as_str().map(|d| cut(d, 1000)).unwrap_or_default(),
                        "model": a["model"].as_str(),
                        "path": p(&file),
                        "scope": "user",
                    }));
                }
            }
            for name in cfg["plugin"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                l.plugins.push(json!({ "name": name, "version": null, "enabled": true, "path": p(&file), "scope": "user" }));
            }
        }
        _ => {
            let base = grok_home();
            for (dirs, name) in [(&mut l.skills, "skills"), (&mut l.agents, "agents"), (&mut l.commands, "commands")] {
                dirs.push((base.join(name), "user"));
                if let Some(pr) = project {
                    dirs.push((pr.join(".grok").join(name), "project"));
                }
            }
            l.rules.push(rules("AGENTS.md", base.join("AGENTS.md"), "user"));
            if let Some(pr) = project {
                l.rules.push(rules("AGENTS.md", pr.join("AGENTS.md"), "project"));
            }
        }
    }
    l
}

fn skills_in((dir, scope): &Dir) -> Vec<Value> {
    names(dir)
        .into_iter()
        .filter_map(|n| {
            let path = dir.join(&n).join("SKILL.md");
            if !is_file(&path) {
                return None;
            }
            let (f, _) = parse_frontmatter(&read_head(&path, HEAD_BYTES));
            let name = Some(field(&f, "name")).filter(|s| !s.is_empty()).unwrap_or(&n);
            Some(json!({ "name": name, "description": cut(field(&f, "description"), 1000), "path": p(&path), "scope": scope }))
        })
        .collect()
}

fn agents_in((dir, scope): &Dir) -> Vec<Value> {
    names(dir)
        .into_iter()
        .filter_map(|n| {
            let path = dir.join(&n);
            let stem = n.strip_suffix(".md")?;
            if !is_file(&path) {
                return None;
            }
            let (f, _) = parse_frontmatter(&read_head(&path, HEAD_BYTES));
            let name = Some(field(&f, "name")).filter(|s| !s.is_empty()).unwrap_or(stem);
            let model = Some(field(&f, "model")).filter(|s| !s.is_empty());
            Some(json!({ "name": name, "description": cut(field(&f, "description"), 1000), "model": model, "path": p(&path), "scope": scope }))
        })
        .collect()
}

fn commands_in((dir, scope): &Dir) -> Vec<Value> {
    let one = |path: &Path, name: String| {
        let (f, body) = parse_frontmatter(&read_head(path, HEAD_BYTES));
        let d = field(&f, "description");
        let description = if d.is_empty() { first_line(&body) } else { d.to_string() };
        json!({ "name": name, "description": cut(&description, 1000), "path": p(path), "scope": scope })
    };
    let mut out = vec![];
    for n in names(dir) {
        let path = dir.join(&n);
        if let Some(stem) = n.strip_suffix(".md")
            && is_file(&path)
        {
            out.push(one(&path, stem.to_string()));
        } else if is_dir(&path) {
            // a sub-folder is a namespace: frontend/review.md → frontend:review
            for m in names(&path) {
                if let Some(stem) = m.strip_suffix(".md")
                    && is_file(&path.join(&m))
                {
                    out.push(one(&path.join(&m), format!("{n}:{stem}")));
                }
            }
        }
    }
    out
}

fn rules_of(l: &Layout) -> Vec<Value> {
    l.rules
        .iter()
        .filter_map(|r| {
            let exists = is_file(&r.path);
            if r.if_exists && !exists {
                return None;
            }
            let size = if exists { std::fs::metadata(&r.path).map(|m| m.len()).unwrap_or(0) } else { 0 };
            let preview =
                if exists { Some(cut(&read_head(&r.path, 4096), 300).trim_end().to_string()).filter(|s| !s.is_empty()) } else { None };
            Some(json!({ "label": r.label, "path": p(&r.path), "scope": r.scope, "exists": exists, "size": size, "preview": preview }))
        })
        .collect()
}

fn extensions_of(kind: &str, project: Option<&Path>) -> Value {
    let l = layout(kind, project);
    let mut agents: Vec<Value> = l.agents.iter().flat_map(agents_in).collect();
    agents.extend(l.more_agents.iter().cloned());
    json!({
        "kind": kind,
        "skills": l.skills.iter().flat_map(skills_in).collect::<Vec<_>>(),
        "rules": rules_of(&l),
        "agents": agents,
        "commands": l.commands.iter().flat_map(commands_in).collect::<Vec<_>>(),
        "plugins": l.plugins,
    })
}

/// Only a rules file the listing names can be opened or saved — never an arbitrary path.
fn listed_rules(kind: &str, project: Option<&Path>, path: Option<&str>) -> ApiResult<PathBuf> {
    let path = path.filter(|p| !p.is_empty()).ok_or_else(|| HttpError::bad_request("path is required"))?;
    let l = layout(kind, project);
    rules_of(&l)
        .iter()
        .find(|r| r["path"] == path)
        .map(|_| PathBuf::from(path))
        .ok_or_else(|| HttpError::new(403, "Not a rules file of this agent"))
}

// ---------- routes ----------

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/harness/mcp", get(mcp_get).post(mcp_post))
        .route("/harness/mcp/{name}", delete(mcp_delete_route))
        .route("/harness/mcp/{name}/agents/{kind}", put(mcp_connect_route).delete(mcp_disconnect_route))
        .route("/harness/{kind}/extensions", get(extensions_route))
        .route("/harness/{kind}/rules", get(rules_get).put(rules_put))
}

#[derive(Deserialize)]
struct ExtQuery {
    project: Option<String>,
    path: Option<String>,
}

/// The folder of `?project=<id>` (None without one).
fn project_dir(s: &AppState, q: &ExtQuery) -> ApiResult<Option<PathBuf>> {
    let Some(id) = q.project.as_deref().filter(|id| !id.is_empty()) else { return Ok(None) };
    let db = s.db()?;
    let p = projects::get(&db, id)?.ok_or_else(|| HttpError::not_found("Project not found"))?;
    Ok(Some(PathBuf::from(p.path)))
}

async fn mcp_get(State(s): State<AppState>, Query(q): Query<ExtQuery>) -> ApiResult<axum::Json<Value>> {
    let dir = project_dir(&s, &q)?;
    Ok(axum::Json(mcp_matrix(dir.as_deref())))
}

async fn mcp_post(State(s): State<AppState>, Query(q): Query<ExtQuery>, JsonBody(b): JsonBody<McpBody>) -> ApiResult<axum::Json<Value>> {
    let dir = project_dir(&s, &q)?;
    Ok(axum::Json(mcp_add(b, dir.as_deref())?))
}

async fn mcp_connect_route(
    State(s): State<AppState>,
    UrlPath((name, kind)): UrlPath<(String, String)>,
    Query(q): Query<ExtQuery>,
) -> ApiResult<axum::Json<Value>> {
    let dir = project_dir(&s, &q)?;
    Ok(axum::Json(mcp_connect(&name, &kind, dir.as_deref())?))
}

async fn mcp_disconnect_route(
    State(s): State<AppState>,
    UrlPath((name, kind)): UrlPath<(String, String)>,
    Query(q): Query<ExtQuery>,
) -> ApiResult<axum::Json<Value>> {
    let dir = project_dir(&s, &q)?;
    Ok(axum::Json(mcp_disconnect(&name, &kind, dir.as_deref())?))
}

async fn mcp_delete_route(
    State(s): State<AppState>,
    UrlPath(name): UrlPath<String>,
    Query(q): Query<ExtQuery>,
) -> ApiResult<axum::Json<Value>> {
    let dir = project_dir(&s, &q)?;
    Ok(axum::Json(mcp_delete(&name, dir.as_deref())?))
}

async fn extensions_route(
    State(s): State<AppState>,
    UrlPath(kind): UrlPath<String>,
    Query(q): Query<ExtQuery>,
) -> ApiResult<axum::Json<Value>> {
    let kind = agent_or_404(&kind)?;
    let dir = project_dir(&s, &q)?;
    Ok(axum::Json(extensions_of(kind, dir.as_deref())))
}

async fn rules_get(State(s): State<AppState>, UrlPath(kind): UrlPath<String>, Query(q): Query<ExtQuery>) -> ApiResult<axum::Json<Value>> {
    let kind = agent_or_404(&kind)?;
    let dir = project_dir(&s, &q)?;
    let path = listed_rules(kind, dir.as_deref(), q.path.as_deref())?;
    if !path.exists() {
        return Ok(axum::Json(json!({ "path": p(&path), "exists": false, "content": "" })));
    }
    if std::fs::metadata(&path)?.len() > MAX_RULES {
        return Err(HttpError::new(413, "The file is too large to edit here"));
    }
    let content = std::fs::read_to_string(&path).map_err(|_| HttpError::new(422, "The file is not UTF-8 text"))?;
    Ok(axum::Json(json!({ "path": p(&path), "exists": true, "content": content })))
}

#[derive(Deserialize)]
struct RulesBody {
    content: String,
}

async fn rules_put(
    State(s): State<AppState>,
    UrlPath(kind): UrlPath<String>,
    Query(q): Query<ExtQuery>,
    JsonBody(b): JsonBody<RulesBody>,
) -> ApiResult<axum::Json<Value>> {
    let kind = agent_or_404(&kind)?;
    let dir = project_dir(&s, &q)?;
    let path = listed_rules(kind, dir.as_deref(), q.path.as_deref())?;
    if b.content.len() as u64 > MAX_RULES {
        return Err(HttpError::new(413, "content: at most 1 MB"));
    }
    write_config(&path, &b.content, false)?;
    Ok(axum::Json(json!({ "path": p(&path), "exists": true, "content": b.content })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_subset() {
        let (f, body) = parse_frontmatter(
            "---\nname: pdf\ndescription: >\n  Read PDFs.\n  Use it.\ntools:\n  - Read\nq: 'it''s'\nc: plain # comment\n---\n# Body\n",
        );
        assert_eq!(field(&f, "name"), "pdf");
        assert_eq!(field(&f, "description"), "Read PDFs. Use it.");
        assert_eq!(field(&f, "tools"), "");
        assert_eq!(field(&f, "q"), "it's");
        assert_eq!(field(&f, "c"), "plain");
        assert_eq!(body, "# Body\n");
        assert!(parse_frontmatter("no frontmatter").0.is_empty());
        assert!(parse_frontmatter("---\nunterminated: yes\n").0.is_empty());
        let (lit, _) = parse_frontmatter("---\nd: |\n  one\n  two\n---\n");
        assert_eq!(field(&lit, "d"), "one\ntwo");
    }

    #[test]
    fn toml_edits_keep_the_rest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("config.toml");
        std::fs::write(&file, "# top\nmodel = \"x\"\n\n[mcp_servers.a]\ncommand = \"a\"\n\n[mcp_servers.a.env]\nK = \"v\"\n\n# keep me\n[profiles.p]\nmodel = \"y\"\n").expect("write");
        let def = McpDef {
            name: "b".into(),
            transport: "http",
            command: None,
            args: vec![],
            env: vec![],
            url: Some("https://b.example".into()),
            headers: vec![("Authorization".into(), "t".into())],
            enabled: true,
        };
        put_def(CODEX, &file, &def).expect("put");
        assert!(remove_def(CODEX, &file, "a").expect("remove"));
        let text = std::fs::read_to_string(&file).expect("read");
        assert!(text.contains("# top") && text.contains("# keep me") && text.contains("[profiles.p]"), "{text}");
        assert!(!text.contains("mcp_servers.a"), "{text}");
        let defs = read_user(CODEX, &file).expect("read");
        assert_eq!(defs, vec![def]);
        assert!(std::fs::read_to_string(dir.path().join("config.toml.hubness.bak")).expect("bak").contains("[mcp_servers.a]"));
        std::fs::write(&file, "broken = \n").expect("write");
        assert_eq!(put_def(CODEX, &file, &defs[0]).map_err(|e| e.status.as_u16()), Err(409));
    }
}
