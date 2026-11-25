//! MCP servers in the agents' own config files: reading each agent's store, the matrix
//! "server × agent", validation, and adding / connecting / disconnecting / deleting servers
//! in the user configs.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Map, Value, json};
use toml_edit::{DocumentMut, Item, Table, value};

use crate::error::{ApiResult, HttpError};
use crate::harness::LABELS;

use super::*;

// ---------- MCP definitions ----------

/// A server with its secrets (env / header values): never sent to clients as is.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct McpDef {
    pub(super) name: String,
    pub(super) transport: &'static str,
    pub(super) command: Option<String>,
    pub(super) args: Vec<String>,
    pub(super) env: Vec<(String, String)>,
    pub(super) url: Option<String>,
    pub(super) headers: Vec<(String, String)>,
    pub(super) enabled: bool,
}

pub(super) fn str_of(v: &Value) -> Option<String> {
    v.as_str().map(String::from)
}

pub(super) fn str_list(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

pub(super) fn str_map(v: &Value) -> Vec<(String, String)> {
    v.as_object().map(|m| m.iter().filter_map(|(k, x)| x.as_str().map(|s| (k.clone(), s.to_string()))).collect()).unwrap_or_default()
}

pub(super) fn map_value(m: &[(String, String)]) -> Value {
    Value::Object(m.iter().map(|(k, v)| (k.clone(), json!(v))).collect())
}

/// Claude Code: `{type, command, args, env}` / `{type, url, headers}`.
pub(super) fn claude_def(name: &str, v: &Value) -> McpDef {
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

pub(super) fn claude_value(d: &McpDef) -> Value {
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
pub(super) fn opencode_def(name: &str, v: &Value) -> McpDef {
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

pub(super) fn opencode_value(d: &McpDef) -> Value {
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

pub(super) fn json_defs(servers: Option<&Value>, kind: &str) -> Vec<McpDef> {
    let Some(m) = servers.and_then(Value::as_object) else { return vec![] };
    m.iter()
        .filter(|(_, v)| v.is_object())
        .map(|(name, v)| if kind == CLAUDE { claude_def(name, v) } else { opencode_def(name, v) })
        .collect()
}

/// The header map of a TOML server: Codex `http_headers`, Grok `headers`.
pub(super) fn headers_key(kind: &str) -> &'static str {
    if kind == CODEX { "http_headers" } else { "headers" }
}

pub(super) fn toml_str(item: Option<&Item>) -> Option<String> {
    item.and_then(|i| i.as_str()).map(String::from)
}

pub(super) fn toml_list(item: Option<&Item>) -> Vec<String> {
    item.and_then(|i| i.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
}

pub(super) fn toml_map(item: Option<&Item>) -> Vec<(String, String)> {
    item.and_then(|i| i.as_table_like())
        .map(|t| t.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.to_string(), s.to_string()))).collect())
        .unwrap_or_default()
}

pub(super) fn toml_defs(doc: &DocumentMut, kind: &str) -> Vec<McpDef> {
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

pub(super) fn toml_table(d: &McpDef, kind: &str) -> Table {
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

pub(super) fn cant_edit(file: &Path) -> HttpError {
    HttpError::new(409, format!("{}: the hub can't change this server without touching the rest of the file — edit it by hand", p(file)))
}

// ---------- one agent's MCP store ----------

pub(super) fn is_toml(kind: &str) -> bool {
    kind == CODEX || kind == GROK
}

pub(super) fn user_file(kind: &str) -> PathBuf {
    match kind {
        CLAUDE => claude_json(),
        CODEX => codex_home().join("config.toml"),
        OPENCODE => opencode_file(),
        _ => grok_home().join("config.toml"),
    }
}

pub(super) fn transports(kind: &str) -> &'static [&'static str] {
    if kind == CODEX { &["stdio", "http"] } else { &["stdio", "http", "sse"] }
}

/// Servers in the user file; [] when it is missing; a 409 when it doesn't parse.
pub(super) fn read_user(kind: &str, file: &Path) -> ApiResult<Vec<McpDef>> {
    if is_toml(kind) {
        return Ok(read_toml_file(file)?.map(|d| toml_defs(&d, kind)).unwrap_or_default());
    }
    let key = if kind == CLAUDE { "mcpServers" } else { "mcp" };
    Ok(read_json_file(file)?.map(|m| json_defs(m.get(key), kind)).unwrap_or_default())
}

/// Servers a project adds (shown, never written); unreadable files are skipped.
pub(super) fn read_project(kind: &str, dir: &Path) -> Vec<(String, Vec<McpDef>)> {
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
pub(super) fn put_def(kind: &str, file: &Path, def: &McpDef) -> ApiResult<()> {
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
pub(super) fn remove_def(kind: &str, file: &Path, name: &str) -> ApiResult<bool> {
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

pub(super) struct Found {
    kind: &'static str,
    scope: &'static str,
    source: String,
    def: McpDef,
}

pub(super) fn scan(project: Option<&Path>) -> (Vec<Found>, Vec<Value>) {
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

pub(super) fn builtin_row() -> Value {
    let agents: Map<String, Value> = LABELS.iter().map(|(k, _, _)| (k.to_string(), json!("builtin"))).collect();
    json!({
        "name": BUILTIN_MCP, "transport": "stdio", "command": null, "args": [], "env": ["HUB_URL", "HUB_SESSION_ID"],
        "url": null, "headers": [], "builtIn": true, "agents": agents, "entries": [],
    })
}

pub(super) fn matrix(found: &[Found], agents: Vec<Value>) -> Value {
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

pub(super) fn mcp_matrix(project: Option<&Path>) -> Value {
    let (found, agents) = scan(project);
    matrix(&found, agents)
}

// ---------- validation ----------

pub(super) fn len(s: &str) -> usize {
    s.encode_utf16().count()
}

pub(super) fn name_ok(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub(super) fn check_name(name: &str) -> ApiResult<()> {
    if !name_ok(name) {
        return Err(HttpError::bad_request("name: letters, digits, - and _ only (at most 64)"));
    }
    if name == BUILTIN_MCP {
        return Err(HttpError::bad_request(format!("“{BUILTIN_MCP}” is the hub's built-in server")));
    }
    Ok(())
}

pub(super) fn env_name_ok(k: &str) -> bool {
    let mut cs = k.chars();
    k.len() <= 128 && cs.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub(super) fn header_name_ok(k: &str) -> bool {
    (1..=128).contains(&k.len()) && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub(super) fn check_map(m: &[(String, String)], ok: fn(&str) -> bool, what: &str) -> ApiResult<()> {
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
pub(super) enum Transport {
    Stdio,
    Http,
    Sse,
}

#[derive(Deserialize)]
pub(super) struct McpBody {
    name: String,
    transport: Transport,
    command: Option<String>,
    args: Option<Vec<String>>,
    env: Option<Map<String, Value>>,
    url: Option<String>,
    headers: Option<Map<String, Value>>,
    agents: Vec<String>,
}

pub(super) fn string_map(m: Option<Map<String, Value>>, what: &str) -> ApiResult<Vec<(String, String)>> {
    m.unwrap_or_default()
        .into_iter()
        .map(|(k, v)| match v {
            Value::String(s) => Ok((k, s)),
            _ => Err(HttpError::bad_request(format!("{what}.{k}: Invalid input: expected string"))),
        })
        .collect()
}

/// A definition from the request, checked; values are kept as given.
pub(super) fn def_of(b: McpBody) -> ApiResult<(McpDef, Vec<String>)> {
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
pub(super) fn supporting(kind: &str, transport: &str) -> ApiResult<&'static str> {
    let (k, label, _) = LABELS.iter().copied().find(|(k, _, _)| *k == kind).ok_or_else(|| HttpError::not_found("Unknown agent"))?;
    if !transports(k).contains(&transport) {
        return Err(HttpError::bad_request(format!("{label} does not support {} MCP servers", transport.to_uppercase())));
    }
    Ok(k)
}

pub(super) fn agent_or_404(kind: &str) -> ApiResult<&'static str> {
    LABELS.iter().map(|(k, _, _)| *k).find(|k| *k == kind).ok_or_else(|| HttpError::not_found("Unknown agent"))
}

// ---------- changes (user configs only) ----------

/// Add a server to (or replace it in) each selected agent. All configs are checked before any
/// is written.
pub(super) fn mcp_add(b: McpBody, project: Option<&Path>) -> ApiResult<Value> {
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
pub(super) fn mcp_connect(name: &str, kind: &str, project: Option<&Path>) -> ApiResult<Value> {
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
pub(super) fn mcp_disconnect(name: &str, kind: &str, project: Option<&Path>) -> ApiResult<Value> {
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
pub(super) fn mcp_delete(name: &str, project: Option<&Path>) -> ApiResult<Value> {
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
