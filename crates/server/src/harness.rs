//! Harness data of the registered agents — a port of `harness.ts`, `agents/kit.ts` and the
//! harness parts of each agent module (`agents/*/index.ts`, `claude/harness.ts`,
//! `claude/models.ts`): the light availability list for pickers, the full per-agent info
//! for the Harness page (versions, update, login, limits, models, default model), provider
//! accounts and default-model settings.
//!
//! Everything is read from official CLI commands and their local files; tokens are never
//! read. The "latest version" checks go to the npm registry, GitHub releases, pacman or the
//! agent's own updater, cached for an hour.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{Path as UrlPath, Query, State};
use axum::routing::{get, post, put};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncWriteExt, BufReader};

use crate::agents::acp::models_key;
use crate::db::{self, kv};
use crate::error::{ApiResult, HttpError};
use crate::http::JsonBody;
use crate::{AppState, agents};

// ---------- kit: running CLIs, versions, the latest-version cache ----------

/// POSIX shell quoting (the TS `shq`).
pub fn shq(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// Run a CLI with stdin closed; stdout on success, `None` on failure or timeout.
pub async fn run_cli(cmd: &str, args: &[&str], timeout: Duration, env: &[(String, String)]) -> Option<String> {
    let mut c = tokio::process::Command::new(cmd);
    c.args(args).envs(env.iter().cloned()).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    let out = tokio::time::timeout(timeout, c.output()).await.ok()?.ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run a command once: exit code and stdout+stderr without ANSI colors (the TS `runOnce`).
pub async fn run_once(cmd: &str, args: &[&str]) -> (Option<i32>, String) {
    let child = tokio::process::Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(Duration::from_secs(60), child).await {
        Ok(Ok(o)) => {
            let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
            (o.status.code(), strip_ansi(&text).trim().to_string())
        }
        Ok(Err(e)) => (None, e.to_string()),
        Err(_) => (None, String::new()),
    }
}

fn strip_ansi(s: &str) -> String {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\x1b\[[0-9;]*m").expect("valid")).replace_all(s, "").into_owned()
}

pub fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The first `x.y.z` in the text.
pub fn parse_version(s: Option<&str>) -> Option<String> {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(\d+\.\d+\.\d+)").expect("valid")).captures(s?).map(|c| c[1].to_string())
}

pub fn cmp_version(a: &str, b: &str) -> std::cmp::Ordering {
    let p = |v: &str| v.split('.').map(|x| x.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    let (pa, pb) = (p(a), p(b));
    for i in 0..3 {
        let (x, y) = (pa.get(i).copied().unwrap_or(0), pb.get(i).copied().unwrap_or(0));
        if x != y {
            return x.cmp(&y);
        }
    }
    std::cmp::Ordering::Equal
}

pub fn version_info(installed: Option<&str>, latest: Option<&str>, source: &str, update: Option<&str>) -> Value {
    let status = match (installed, latest) {
        (None, _) => "not-installed",
        (Some(_), None) => "unknown",
        (Some(i), Some(l)) if cmp_version(i, l).is_lt() => "update-available",
        _ => "up-to-date",
    };
    json!({ "installed": installed, "latest": latest, "status": status, "source": source, "updateCommand": update })
}

pub fn action(id: &str, command: &str, runnable: bool, interactive: bool) -> Value {
    json!({ "id": id, "command": command, "runnable": runnable, "interactive": interactive })
}

fn act(id: &str, command: &str) -> Value {
    action(id, command, true, true)
}

pub fn not_found(bin: &str) -> String {
    format!("Not installed (“{bin}” not found) — see Harness")
}

type LatestCache = Mutex<HashMap<String, (Instant, Option<String>)>>;

fn latest_cache() -> &'static LatestCache {
    static C: OnceLock<LatestCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Latest versions from the network, per agent kind, cached for an hour (failures too).
async fn cached_latest<F: std::future::Future<Output = Option<String>>>(
    kind: &str,
    key: &str,
    fetch: impl FnOnce() -> F,
) -> Option<String> {
    let k = format!("{kind}\0{key}");
    if let Some((at, v)) = latest_cache().lock().expect("latest cache").get(&k)
        && at.elapsed() < Duration::from_secs(3600)
    {
        return v.clone();
    }
    let v = fetch().await;
    latest_cache().lock().expect("latest cache").insert(k, (Instant::now(), v.clone()));
    v
}

/// Forget the cached latest versions of one agent (its refresh, or after an update).
fn clear_latest(kind: &str) {
    latest_cache().lock().expect("latest cache").retain(|k, _| !k.starts_with(&format!("{kind}\0")));
}

fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder().timeout(Duration::from_secs(10)).user_agent("hubness").build().expect("http client"))
}

async fn npm_latest(pkg: &str) -> Option<String> {
    let res = http().get(format!("https://registry.npmjs.org/{pkg}/latest")).send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    let v: Value = res.json().await.ok()?;
    parse_version(v["version"].as_str())
}

/// Full path of a binary found on PATH, with symlinks resolved.
fn resolve_bin(bin: &str) -> Option<PathBuf> {
    let found = if bin.contains('/') {
        Some(PathBuf::from(bin)).filter(|p| p.is_file())
    } else {
        std::env::var("PATH").ok()?.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join(bin)).find(|p| p.is_file())
    }?;
    Some(std::fs::canonicalize(&found).unwrap_or(found))
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()))
}

fn env_set(k: &str) -> bool {
    std::env::var(k).is_ok_and(|v| !v.is_empty())
}

// ---------- Claude Code ----------

const CLAUDE: &str = "claude-code";

fn claude_bin() -> String {
    agents::claude::claude_bin()
}

fn claude_json() -> PathBuf {
    home().join(".claude.json")
}

fn claude_settings() -> PathBuf {
    home().join(".claude").join("settings.json")
}

async fn claude_version() -> Option<String> {
    run_cli(&claude_bin(), &["--version"], Duration::from_secs(10), &[]).await.map(|v| v.trim().to_string())
}

/// Cheap check for pickers: the CLI keeps the signed-in account in ~/.claude.json.
fn claude_signed_in() -> bool {
    read_json(&claude_json()).is_some_and(|j| j.get("oauthAccount").is_some_and(|o| !o.is_null() && o != &json!(false)))
        || env_set("ANTHROPIC_API_KEY")
}

const MODELS_KEY: &str = "claude.models";
const MODELS_TTL: Duration = Duration::from_secs(6 * 3600);

fn fallback_models() -> Vec<Value> {
    vec![json!({ "id": "opus", "label": "Opus" }), json!({ "id": "sonnet", "label": "Sonnet" }), json!({ "id": "haiku", "label": "Haiku" })]
}

/// Models as the CLI lists them (the /model picker): the `initialize` control request of
/// stream-json mode returns `models`. No prompt is sent, so no tokens are spent.
async fn read_claude_models() -> Option<Vec<Value>> {
    let mut child = tokio::process::Command::new(claude_bin())
        .args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"])
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let req = json!({ "type": "control_request", "request_id": "hub-models", "request": { "subtype": "initialize" } });
    stdin.write_all(format!("{req}\n").as_bytes()).await.ok()?;
    let stdout = child.stdout.take()?;
    let read = async move {
        use tokio::io::AsyncBufReadExt;
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(j) = serde_json::from_str::<Value>(&line) else { continue };
            if j["type"] != "control_response" {
                continue;
            }
            let r = if j["response"]["response"].is_object() { &j["response"]["response"] } else { &j["response"] };
            let models = r["models"].as_array()?;
            return Some(
                models
                    .iter()
                    .filter(|m| m["value"].is_string())
                    .map(|m| {
                        let s = |k: &str| m[k].as_str().filter(|v| !v.is_empty()).map(String::from);
                        let mut o = Map::new();
                        o.insert("id".into(), m["value"].clone());
                        o.insert(
                            "label".into(),
                            json!(s("displayName").unwrap_or_else(|| m["value"].as_str().unwrap_or_default().to_string())),
                        );
                        if let Some(d) = s("description") {
                            o.insert("description".into(), json!(d));
                        }
                        if let Some(r) = s("resolvedModel") {
                            o.insert("resolved".into(), json!(r));
                        }
                        Value::Object(o)
                    })
                    .collect(),
            );
        }
        None
    };
    let result = tokio::time::timeout(Duration::from_secs(30), read).await.ok().flatten();
    drop(stdin);
    let _ = child.kill().await;
    result
}

/// Models offered by the installed Claude Code (cached 6 hours; refresh re-reads).
async fn claude_model_list(s: &AppState, refresh: bool) -> Vec<Value> {
    let saved = s.db().ok().and_then(|db| kv_with_time(&db, MODELS_KEY));
    if !refresh
        && let Some((v, at)) = &saved
        && fresh(at, MODELS_TTL)
    {
        return v.as_array().cloned().unwrap_or_default();
    }
    match read_claude_models().await.filter(|l| !l.is_empty()) {
        Some(list) => {
            if let Ok(db) = s.db() {
                let _ = kv::set(&db, MODELS_KEY, &list);
            }
            list
        }
        None => saved.and_then(|(v, _)| v.as_array().cloned()).unwrap_or_else(fallback_models),
    }
}

fn kv_with_time(db: &rusqlite::Connection, key: &str) -> Option<(Value, String)> {
    db.query_row("SELECT value, updated_at FROM kv WHERE key = ?", [key], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .ok()
        .and_then(|(v, t)| serde_json::from_str(&v).ok().map(|v| (v, t)))
}

fn fresh(iso: &str, ttl: Duration) -> bool {
    chrono::DateTime::parse_from_rfc3339(iso)
        .is_ok_and(|t| (chrono::Utc::now().timestamp_millis() - t.timestamp_millis()) < ttl.as_millis() as i64)
}

fn human_tier(tier: Option<&str>) -> Option<String> {
    let tier = tier.filter(|t| !t.is_empty())?;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    if let Some(c) = RE.get_or_init(|| regex::Regex::new(r"max_(\d+x)").expect("valid")).captures(tier) {
        return Some(format!("Max {}", &c[1]));
    }
    Some(tier.strip_prefix("default_").unwrap_or(tier).replace('_', " "))
}

async fn claude_auth() -> Option<Value> {
    let out = run_cli(&claude_bin(), &["auth", "status"], Duration::from_secs(20), &[]).await?;
    let st: Value = serde_json::from_str(&out).unwrap_or_else(|_| json!({}));
    let oa = read_json(&claude_json()).map(|j| j["oauthAccount"].clone()).filter(Value::is_object).unwrap_or_else(|| json!({}));
    let s = |v: &Value| v.as_str().map(String::from);
    let sub = s(&st["subscriptionType"]);
    let plan = human_tier(oa["userRateLimitTier"].as_str().or(oa["organizationRateLimitTier"].as_str())).or_else(|| {
        sub.map(|v| {
            let mut c = v.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
    });
    Some(json!({
        "loggedIn": st["loggedIn"].as_bool().unwrap_or(false),
        "method": st["authMethod"].as_str().filter(|m| !m.is_empty() && *m != "none"),
        "plan": plan,
        "planDetail": s(&oa["planDisplayName"]),
        "email": s(&st["email"]).or_else(|| s(&oa["emailAddress"])),
        "org": s(&st["orgName"]).or_else(|| s(&oa["organizationName"])),
        "expiresAt": s(&st["loginExpiresAt"]),
        "extraUsage": oa["hasExtraUsageEnabled"].as_bool(),
    }))
}

/// Limits from the CLI's own cache (.claude.json → cachedUsageUtilization), what /usage shows.
fn cli_cached_limits(claude_json: Option<&Value>) -> Option<(Vec<Value>, String)> {
    let c = &claude_json?["cachedUsageUtilization"];
    let u = c.get("utilization").filter(|u| u.is_object())?;
    let mut limits: Vec<Value> = u["limits"]
        .as_array()
        .map(|ls| {
            ls.iter()
                .filter(|l| l["percent"].is_number() && l["kind"].is_string())
                .map(|l| {
                    let sc = &l["scope"];
                    let scope = sc["model"]["display_name"].as_str().or(sc["surface"]["display_name"].as_str()).or(sc["surface"].as_str()).map(String::from);
                    let kind = l["kind"].as_str().unwrap_or_default();
                    let id = match &scope {
                        Some(s) => format!("{kind}:{s}"),
                        None => kind.to_string(),
                    };
                    json!({ "id": id, "group": l.get("group").cloned().unwrap_or(Value::Null), "scope": scope, "utilization": l["percent"].as_f64().unwrap_or(0.0).round() as i64, "resetsAt": l.get("resets_at").cloned().unwrap_or(Value::Null) })
                })
                .collect()
        })
        .unwrap_or_default();
    if limits.is_empty() {
        limits = ["five_hour", "seven_day", "seven_day_opus", "seven_day_sonnet"]
            .iter()
            .filter(|id| u[**id]["utilization"].is_number())
            .map(|id| json!({ "id": id, "utilization": u[*id]["utilization"].as_f64().unwrap_or(0.0).round() as i64, "resetsAt": u[*id].get("resets_at").cloned().unwrap_or(Value::Null) }))
            .collect();
    }
    let at = chrono::DateTime::from_timestamp_millis(c["fetchedAtMs"].as_i64().unwrap_or(0))?
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    (!limits.is_empty()).then_some((limits, at))
}

/// Our rate_limit_event windows → the CLI limit they update.
fn window_matches(ours: &str, l: &Value) -> Option<bool> {
    match ours {
        "seven_day" => Some(l["id"] == "seven_day" || l["id"] == "weekly_all"),
        "five_hour" => Some(l["id"] == "five_hour" || (l["group"] == "session" && (l["scope"].is_null() || l["scope"] == ""))),
        _ => None,
    }
}

/// The CLI list (all windows) with fresher numbers from our own turns where they match.
fn merge_limits(cli: Option<(Vec<Value>, String)>, ours: Option<(Value, String)>) -> (Vec<Value>, Option<String>) {
    let ours_list = ours.as_ref().and_then(|(v, _)| v.as_array().cloned()).unwrap_or_default();
    let Some((cli_list, cli_at)) = cli else { return (ours_list, ours.map(|(_, t)| t)) };
    let Some((_, ours_at)) = ours.filter(|(_, t)| *t > cli_at) else { return (cli_list, Some(cli_at)) };
    let mut limits: Vec<Value> = cli_list
        .iter()
        .map(|l| match ours_list.iter().find(|o| window_matches(o["id"].as_str().unwrap_or_default(), l) == Some(true)) {
            Some(w) => {
                let mut m = l.clone();
                m["utilization"] = w["utilization"].clone();
                if !w["resetsAt"].is_null() {
                    m["resetsAt"] = w["resetsAt"].clone();
                }
                m
            }
            None => l.clone(),
        })
        .collect();
    // a window we saw that the CLI cache doesn't have yet
    for o in &ours_list {
        let id = o["id"].as_str().unwrap_or_default();
        if !cli_list.iter().any(|l| window_matches(id, l).unwrap_or(l["id"] == o["id"])) {
            limits.push(o.clone());
        }
    }
    (limits, Some(ours_at))
}

/// The headline limit goes first: the UI shows the first one in the stats row.
fn main_first(mut limits: Vec<Value>) -> Vec<Value> {
    if let Some(i) = limits.iter().position(|w| w["id"] == "weekly_all" || w["id"] == "seven_day")
        && i > 0
    {
        let w = limits.remove(i);
        limits.insert(0, w);
    }
    limits
}

fn claude_default_model() -> Option<String> {
    read_json(&claude_settings()).and_then(|j| j["model"].as_str().filter(|m| !m.is_empty()).map(String::from))
}

/// Set / clear `model` in ~/.claude/settings.json; the rest of the file is untouched. Written
/// atomically through the real path (a symlinked settings.json stays a symlink).
fn set_claude_default_model(model: Option<&str>) -> ApiResult<()> {
    if let Some(m) = model
        && !(m.len() <= 100 && !m.is_empty() && m.chars().all(|c| c.is_ascii_alphanumeric() || "_.:[]-".contains(c)))
    {
        return Err(HttpError::bad_request("Bad model id"));
    }
    write_json_key(&claude_settings(), "model", model, None, "settings.json")
}

fn write_json_key(path: &Path, key: &str, value: Option<&str>, init: Option<Value>, label: &str) -> ApiResult<()> {
    let mut cfg = if path.exists() {
        read_json(path)
            .filter(Value::is_object)
            .ok_or_else(|| HttpError::new(409, format!("{label} is not plain JSON — edit it by hand")))?
    } else {
        init.unwrap_or_else(|| json!({}))
    };
    let obj = cfg.as_object_mut().expect("an object");
    match value {
        Some(v) => {
            obj.insert(key.into(), json!(v));
        }
        None => {
            obj.shift_remove(key);
        }
    }
    let real = if path.exists() { std::fs::canonicalize(path)? } else { path.to_path_buf() };
    if let Some(parent) = real.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = real.with_file_name(format!("{}.hub-{}.tmp", crate::paths::basename(&real), std::process::id()));
    std::fs::write(&tmp, serde_json::to_string_pretty(&cfg).expect("json") + "\n")?;
    std::fs::rename(&tmp, &real)?;
    Ok(())
}

/// Update command by install method: the binary's npm prefix or its own updater.
fn claude_update_command(real: Option<&Path>) -> String {
    let real = real.map(crate::paths::to_string).unwrap_or_default();
    match real.find("/lib/node_modules/@anthropic-ai/claude-code/") {
        Some(i) => format!("npm install -g --prefix {} @anthropic-ai/claude-code@latest", shq(&real[..i])),
        None => format!("{} update", shq(&claude_bin())),
    }
}

async fn claude_harness(s: &AppState, refresh: bool) -> Value {
    let bin = claude_bin();
    let installed = parse_version(run_cli(&bin, &["--version"], Duration::from_secs(20), &[]).await.as_deref());
    let real = resolve_bin(&bin);
    let latest = cached_latest(CLAUDE, "npm", || npm_latest("@anthropic-ai/claude-code")).await;
    let real_s = real.as_deref().map(crate::paths::to_string);
    let npm_prefix = real_s.as_deref().and_then(|r| r.find("/lib/node_modules/").map(|i| r[..i].to_string()));
    let update = claude_update_command(real.as_deref());
    let source = npm_prefix.map(|p| format!("npm ({p})")).unwrap_or_else(|| "native installer".into());
    let version = version_info(installed.as_deref(), latest.as_deref(), &source, Some(&update));
    let b = shq(&bin);
    let actions = if installed.is_some() {
        vec![
            act("update", &update),
            act("login", &format!("{b} auth login")),
            act("login-console", &format!("{b} auth login --console")),
            act("logout", &format!("{b} auth logout")),
        ]
    } else {
        vec![act("install", "npm install -g @anthropic-ai/claude-code@latest")]
    };
    let auth = if installed.is_some() { claude_auth().await } else { None };
    let accounts = if installed.is_some() {
        let a = auth.clone().unwrap_or(Value::Null);
        vec![
            json!({ "id": "default", "label": "claude.ai", "detail": a["email"], "loggedIn": a["loggedIn"].as_bool().unwrap_or(false), "active": true }),
        ]
    } else {
        vec![]
    };
    let ours = s.db().ok().and_then(|db| kv_with_time(&db, "claude.limits"));
    let (limits, as_of) = merge_limits(cli_cached_limits(read_json(&claude_json()).as_ref()), ours);
    let models = if installed.is_some() { claude_model_list(s, refresh).await } else { vec![] };
    json!({
        "kind": CLAUDE, "label": "Claude Code", "protocol": "cli", "pinned": false,
        "version": version, "actions": actions, "accounts": accounts, "auth": auth,
        "accountMode": "single", "limits": main_first(limits), "limitsAsOf": as_of,
        "models": models, "defaultModel": claude_default_model(), "defaultModelEdit": "options", "usage": null,
    })
}

// ---------- Codex ----------

const CODEX: &str = "codex";
const CODEX_ACP_VERSION: &str = "0.16.0";
const CODEX_CLI_VERSION: &str = "0.166.0";
const CODEX_ACP_PKG: &str = "@zed-industries/codex-acp";
const CODEX_CLI_PKG: &str = "@openai/codex";

fn codex_override() -> Option<Vec<String>> {
    std::env::var("HUBNESS_CODEX_ACP").ok().map(|v| v.split_whitespace().map(String::from).collect::<Vec<_>>()).filter(|v| !v.is_empty())
}

/// Version of the npm package a global bin links to (…/node_modules/<pkg>/bin/x.js).
fn bin_version(bin: &str, pkg: &str) -> Option<String> {
    let real = resolve_bin(bin)?;
    let mut dir = real.parent()?.to_path_buf();
    for _ in 0..4 {
        if let Some(j) = read_json(&dir.join("package.json"))
            && j["name"] == pkg
        {
            return j["version"].as_str().map(String::from);
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}

/// Installed (adapter, cli) versions; a custom adapter command counts as installed.
fn codex_installed() -> (Option<String>, Option<String>) {
    let acp = if codex_override().is_some() { Some("custom".to_string()) } else { bin_version("codex-acp", CODEX_ACP_PKG) };
    (acp, bin_version("codex", CODEX_CLI_PKG))
}

/// npm -g writes to <node prefix>/lib/node_modules; without write access it needs root.
fn npm_global_writable() -> bool {
    let Some(node) = resolve_bin("node") else { return false };
    let Some(prefix) = node.parent().and_then(Path::parent) else { return false };
    let dir = prefix.join("lib").join("node_modules");
    let probe = dir.join(format!(".hubness-write-check-{}", std::process::id()));
    let ok = std::fs::File::create(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

fn codex_install_command(acp: &str) -> String {
    format!("npm i -g {CODEX_ACP_PKG}@{acp} {CODEX_CLI_PKG}@{CODEX_CLI_VERSION}")
}

fn codex_dir() -> PathBuf {
    std::env::var("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|_| home().join(".codex"))
}

/// How Codex is authenticated: ChatGPT subscription (auth.json) or an API key.
fn codex_auth() -> Option<&'static str> {
    if codex_dir().join("auth.json").exists() {
        Some("ChatGPT / auth.json")
    } else if env_set("CODEX_API_KEY") {
        Some("CODEX_API_KEY")
    } else if env_set("OPENAI_API_KEY") {
        Some("OPENAI_API_KEY")
    } else {
        None
    }
}

fn kv_models(s: &AppState, kind: &str) -> Vec<String> {
    s.db().ok().and_then(|db| kv::get::<Vec<String>>(&db, &models_key(kind)).ok().flatten()).unwrap_or_default()
}

/// The adapter runs sessions, the CLI signs in: both are needed.
fn codex_ready() -> (Option<String>, bool, Option<String>) {
    let (acp, cli) = codex_installed();
    let custom = acp.as_deref() == Some("custom");
    let installed = if custom || cli.is_some() { acp.clone() } else { None };
    (acp, custom, installed)
}

async fn codex_harness(s: &AppState) -> Value {
    let (_, custom, installed) = codex_ready();
    let writable = npm_global_writable();
    let latest = if installed.is_some() && !custom { cached_latest(CODEX, "npm", || npm_latest(CODEX_ACP_PKG)).await } else { None };
    let update = latest.as_deref().map(codex_install_command);
    let method = installed.as_ref().and_then(|_| codex_auth());
    let version = if custom {
        let cmd = codex_override().unwrap_or_default().join(" ");
        json!({ "installed": null, "latest": null, "status": "unknown", "source": format!("HUBNESS_CODEX_ACP ({cmd})"), "updateCommand": null })
    } else {
        version_info(
            installed.as_deref(),
            latest.as_deref(),
            if installed.is_some() { "npm -g (codex-acp)" } else { "—" },
            update.as_deref(),
        )
    };
    let actions = if installed.is_some() {
        let mut a = vec![];
        if let Some(u) = update.as_deref().filter(|_| !custom) {
            a.push(action("update", u, writable, true));
        }
        a.push(act("login", "codex login --device-auth"));
        a.push(act("logout", "codex logout"));
        a
    } else {
        vec![action("install", &codex_install_command(CODEX_ACP_VERSION), writable, true)]
    };
    let accounts = if installed.is_some() {
        vec![json!({ "id": "codex", "label": "OpenAI", "detail": method, "loggedIn": method.is_some(), "active": true })]
    } else {
        vec![]
    };
    let models: Vec<Value> = kv_models(s, CODEX).into_iter().map(|id| json!({ "id": id, "label": id })).collect();
    json!({
        "kind": CODEX, "label": "Codex", "protocol": "acp", "pinned": true,
        "version": version, "actions": actions, "accounts": accounts,
        "auth": { "loggedIn": method.is_some(), "method": method },
        "accountMode": "single", "limits": null, "limitsAsOf": null,
        "models": models, "defaultModel": null, "defaultModelEdit": null, "usage": null,
    })
}

// ---------- OpenCode ----------

const OPENCODE: &str = "opencode";

fn opencode_bin() -> String {
    std::env::var("HUBNESS_OPENCODE_BIN").ok().filter(|b| !b.is_empty()).unwrap_or_else(|| "opencode".into())
}

fn oc_config_dir() -> PathBuf {
    home().join(".config").join("opencode")
}

async fn opencode_version() -> Option<String> {
    run_cli(&opencode_bin(), &["--version"], Duration::from_secs(10), &[]).await.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

struct OcProvider {
    id: String,
    name: String,
    connections: Vec<Value>,
}

async fn opencode_providers() -> Vec<OcProvider> {
    let out = run_cli(&opencode_bin(), &["auth", "list", "--format", "json"], Duration::from_secs(15), &[]).await;
    let Ok(Value::Array(list)) = serde_json::from_str::<Value>(out.as_deref().unwrap_or("[]")) else { return vec![] };
    let str_of = |v: &Value| v.as_str().map(String::from).unwrap_or_else(|| v.to_string());
    list.iter()
        .map(|p| OcProvider {
            id: str_of(&p["id"]),
            name: if p["name"].is_null() { str_of(&p["id"]) } else { str_of(&p["name"]) },
            connections: p["connections"]
                .as_array()
                .map(|cs| {
                    cs.iter()
                        .map(|c| {
                            let label = [&c["label"], &c["method"], &c["id"]].into_iter().find(|v| !v.is_null()).map(str_of).unwrap_or_default();
                            let mut o = json!({ "id": str_of(&c["id"]), "label": label, "method": if c["method"].is_null() { String::new() } else { str_of(&c["method"]) } });
                            if let Some(a) = c["active"].as_bool() {
                                o["active"] = json!(a);
                            }
                            o
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect()
}

type ModelsCache = Mutex<Option<(Instant, Vec<String>)>>;

/// Models available in opencode (`opencode models`), cached for 10 minutes; with parallel
/// calls opencode sometimes returns an empty list: retry, and don't cache empty results.
async fn opencode_models() -> Vec<String> {
    static CACHE: OnceLock<ModelsCache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((at, m)) = cache.lock().expect("models cache").as_ref()
        && at.elapsed() < Duration::from_secs(600)
    {
        return m.clone();
    }
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"^[\w.~-]+/\S+$").expect("valid"));
    for _ in 0..3 {
        let Some(out) = run_cli(&opencode_bin(), &["models"], Duration::from_secs(30), &[]).await else { break };
        let models: Vec<String> = out.lines().map(str::trim).filter(|l| re.is_match(l)).map(String::from).collect();
        if !models.is_empty() {
            *cache.lock().expect("models cache") = Some((Instant::now(), models.clone()));
            return models;
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    cache.lock().expect("models cache").as_ref().map(|(_, m)| m.clone()).unwrap_or_default()
}

/// Models of connected providers only: the others won't run anyway.
async fn connected_models(providers: Option<&[OcProvider]>) -> Vec<String> {
    let owned;
    let list = match providers {
        Some(p) => p,
        None => {
            owned = opencode_providers().await;
            &owned
        }
    };
    let all = opencode_models().await;
    if list.is_empty() {
        return all;
    }
    let mut ids: Vec<String> = list.iter().filter(|p| !p.connections.is_empty()).map(|p| p.id.clone()).collect();
    if let Some(cfg) = read_json(&oc_config_dir().join("opencode.json"))
        && let Some(prov) = cfg["provider"].as_object()
    {
        ids.extend(prov.keys().cloned());
    }
    all.into_iter().filter(|m| ids.iter().any(|i| m.split('/').next() == Some(i))).collect()
}

fn opencode_default_model() -> Option<String> {
    for f in ["opencode.json", "opencode.jsonc"] {
        if let Some(cfg) = read_json(&oc_config_dir().join(f))
            && let Some(m) = cfg.get("model").filter(|m| !m.is_null() && *m != "")
        {
            return Some(m.as_str().map(String::from).unwrap_or_else(|| m.to_string()));
        }
    }
    let state = read_json(&home().join(".local/state/opencode/model.json"))?;
    let r = &state["recent"][0];
    (!r.is_null()).then(|| format!("{}/{}", r["providerID"].as_str().unwrap_or_default(), r["modelID"].as_str().unwrap_or_default()))
}

fn set_opencode_default_model(model: Option<&str>) -> ApiResult<()> {
    if let Some(m) = model {
        let ok = m.split_once('/').is_some_and(|(p, rest)| {
            !p.is_empty()
                && p.chars().all(|c| c.is_alphanumeric() || "_.~-".contains(c))
                && !rest.is_empty()
                && !rest.chars().any(char::is_whitespace)
        });
        if !ok {
            return Err(HttpError::bad_request("Model must look like provider/model"));
        }
    }
    let init = json!({ "$schema": "https://opencode.ai/config.json" });
    write_json_key(&oc_config_dir().join("opencode.json"), "model", model, Some(init), "opencode.json")
}

fn parse_stats(out: Option<&str>) -> Value {
    let Some(out) = out else { return Value::Null };
    let clean = strip_ansi(out);
    let num = |re: &str| regex::Regex::new(re).expect("valid").captures(&clean).and_then(|c| c[1].replace(',', "").parse::<f64>().ok());
    let tokens = regex::Regex::new(r"([\d.,]+[kKmMbB]?)\s+tokens").expect("valid").captures(&clean).map(|c| c[1].to_string());
    let sessions = num(r"([\d,]+)\s+sessions?");
    let prompts = num(r"([\d,]+)\s+prompts?");
    if sessions.is_none() && prompts.is_none() && tokens.is_none() {
        return Value::Null;
    }
    let n = |v: Option<f64>| v.map(|f| json!(f as i64)).unwrap_or(Value::Null);
    json!({ "sessions": n(sessions), "prompts": n(prompts), "tokens": tokens })
}

async fn opencode_source(real: Option<&Path>) -> (Option<String>, &'static str, bool) {
    // installed by a package manager — check against its repository
    if let Some(r) = real {
        let owner = run_cli("pacman", &["-Qqo", &crate::paths::to_string(r)], Duration::from_secs(5), &[]).await;
        if let Some(pkg) = owner.as_deref().map(str::trim).filter(|o| !o.is_empty()).and_then(|o| o.lines().next()).map(String::from) {
            let latest = cached_latest(OPENCODE, &format!("pacman:{pkg}"), || async {
                let si = run_cli("pacman", &["-Si", &pkg], Duration::from_secs(10), &[]).await?;
                let v = regex::Regex::new(r"(?m)^Version\s*:\s*(\S+)").expect("valid").captures(&si).map(|c| c[1].to_string());
                parse_version(v.as_deref())
            })
            .await;
            return (latest, "pacman", true);
        }
    }
    let latest = cached_latest(OPENCODE, "github", || async {
        let res = http()
            .get("https://api.github.com/repos/anomalyco/opencode/releases?per_page=10")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .ok()?;
        if !res.status().is_success() {
            return None;
        }
        let list: Vec<Value> = res.json().await.ok()?;
        let mut versions: Vec<String> = list.iter().filter_map(|r| parse_version(r["tag_name"].as_str())).collect();
        versions.sort_by(|a, b| cmp_version(a, b));
        versions.pop()
    })
    .await;
    (latest, "GitHub releases", false)
}

/// Update opencode by install method (`opencode upgrade` knows only its own installers; a
/// manually placed binary is replaced with a fresh archive from GitHub releases).
fn opencode_upgrade_command(real: Option<&Path>, latest: Option<&str>) -> String {
    let bin = shq(&opencode_bin());
    let v = latest.map(|l| format!(" {l}")).unwrap_or_default();
    let Some(real) = real.map(crate::paths::to_string) else { return format!("{bin} upgrade{v}") };
    for (needle, method) in
        [("/.opencode/bin/", "curl"), ("/.bun/", "bun"), ("/pnpm/", "pnpm"), ("/node_modules/", "npm"), ("brew", "brew")]
    {
        if real.contains(needle) {
            return format!("{bin} upgrade{v} --method {method}");
        }
    }
    let tag = latest.map(|l| format!("download/v{l}")).unwrap_or_else(|| "latest/download".into());
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };
    let dir = real.rfind('/').map(|i| &real[..i]).filter(|d| !d.is_empty()).unwrap_or("/");
    format!(
        "curl -fsSL https://github.com/anomalyco/opencode/releases/{tag}/opencode-linux-{arch}.tar.gz | tar -xz -C {} opencode",
        shq(dir)
    )
}

async fn opencode_harness() -> Value {
    let bin = opencode_bin();
    let real = resolve_bin(&bin);
    let installed = parse_version(run_cli(&bin, &["--version"], Duration::from_secs(20), &[]).await.as_deref());
    let (latest, source, pacman) = opencode_source(real.as_deref()).await;
    let update =
        if pacman { "sudo pacman -Syu opencode".to_string() } else { opencode_upgrade_command(real.as_deref(), latest.as_deref()) };
    let version = version_info(installed.as_deref(), latest.as_deref(), if installed.is_some() { source } else { "—" }, Some(&update));
    let mut info = json!({
        "kind": OPENCODE, "label": "Opencode", "protocol": "acp", "pinned": false, "version": version,
        "auth": null, "accountMode": "providers", "limits": null, "limitsAsOf": null, "defaultModelEdit": "search",
    });
    let o = info.as_object_mut().expect("object");
    if installed.is_none() {
        o.insert(
            "actions".into(),
            json!([
                act("install", "curl -fsSL https://opencode.ai/install | bash"),
                action("install", "sudo pacman -S opencode", false, true)
            ]),
        );
        o.insert("accounts".into(), json!([]));
        o.insert("defaultModel".into(), Value::Null);
        o.insert("models".into(), json!([]));
        o.insert("usage".into(), Value::Null);
        return info;
    }
    // opencode commands run one at a time: in parallel the CLI sometimes returns empty output
    let providers = opencode_providers().await;
    let stats = if providers.is_empty() { None } else { run_cli(&bin, &["stats"], Duration::from_secs(15), &[]).await };
    let models = connected_models(Some(&providers)).await;
    let accounts: Vec<Value> = providers
        .iter()
        .flat_map(|p| {
            let single = p.connections.len() == 1;
            p.connections.iter().map(move |c| {
                json!({
                    "id": c["id"], "label": c["label"],
                    "detail": c["method"].as_str().filter(|m| !m.is_empty()),
                    "loggedIn": true, "active": c["active"].as_bool().unwrap_or(single),
                    "provider": p.id, "providerName": p.name,
                })
            })
        })
        .collect();
    o.insert("actions".into(), json!([action("update", &update, !pacman, true), act("login", &format!("{} auth login", shq(&bin)))]));
    o.insert("accounts".into(), json!(accounts));
    o.insert("defaultModel".into(), json!(opencode_default_model()));
    o.insert("models".into(), json!(models.iter().map(|id| json!({ "id": id, "label": id })).collect::<Vec<_>>()));
    o.insert("usage".into(), parse_stats(stats.as_deref()));
    info
}

// ---------- Grok ----------

const GROK: &str = "grok";

fn grok_bin() -> String {
    std::env::var("HUBNESS_GROK_BIN").ok().filter(|b| !b.is_empty()).unwrap_or_else(|| "grok".into())
}

async fn grok_version() -> Option<String> {
    parse_version(run_cli(&grok_bin(), &["--version"], Duration::from_secs(10), &[]).await.as_deref())
}

fn grok_logged_in() -> bool {
    home().join(".grok").join("auth.json").exists()
}

/// Models from the CLI's own cache (~/.grok/models_cache.json), otherwise those declared in
/// the last session.
fn grok_models(s: &AppState) -> Vec<String> {
    let cached: Vec<String> = read_json(&home().join(".grok/models_cache.json"))
        .and_then(|d| d["models"].as_object().map(|m| m.keys().cloned().collect()))
        .unwrap_or_default();
    if cached.is_empty() { kv_models(s, GROK) } else { cached }
}

async fn grok_harness(s: &AppState) -> Value {
    let installed = grok_version().await;
    let logged_in = installed.is_some() && grok_logged_in();
    let latest = if installed.is_some() {
        cached_latest(GROK, "self", || async {
            let out = run_cli(&grok_bin(), &["update", "--check", "--json"], Duration::from_secs(30), &[]).await;
            let v: Value = serde_json::from_str(out.as_deref().unwrap_or("{}")).unwrap_or_else(|_| json!({}));
            parse_version(v["latestVersion"].as_str())
        })
        .await
    } else {
        None
    };
    let b = shq(&grok_bin());
    let update = format!("{b} update");
    let actions = if installed.is_some() {
        vec![act("update", &update), act("login", &format!("{b} login --device-auth")), act("logout", &format!("{b} logout"))]
    } else {
        vec![act("install", "curl -fsSL https://x.ai/cli/install.sh | bash")]
    };
    let accounts = if installed.is_some() {
        vec![
            json!({ "id": "grok", "label": "grok.com", "detail": if logged_in { Some("session") } else { None }, "loggedIn": logged_in, "active": true }),
        ]
    } else {
        vec![]
    };
    json!({
        "kind": GROK, "label": "Grok CLI", "protocol": "acp", "pinned": false,
        "version": version_info(installed.as_deref(), latest.as_deref(), if installed.is_some() { "grok installer" } else { "—" }, installed.as_ref().map(|_| update.as_str())),
        "actions": actions, "accounts": accounts,
        "auth": { "loggedIn": logged_in, "method": if logged_in { Some("grok.com account") } else { None } },
        "accountMode": "single", "limits": null, "limitsAsOf": null,
        "models": grok_models(s).into_iter().map(|id| json!({ "id": id, "label": id })).collect::<Vec<_>>(),
        "defaultModel": null, "defaultModelEdit": null, "usage": null,
    })
}

// ---------- registry: status, models, harness ----------

/// Registered agents in UI order (the order of the TS registry).
const LABELS: &[(&str, &str, &str)] =
    &[(OPENCODE, "Opencode", "search"), (CLAUDE, "Claude Code", "options"), (GROK, "Grok CLI", "search"), (CODEX, "Codex", "search")];

fn label_of(kind: &str) -> ApiResult<(&'static str, &'static str, &'static str)> {
    LABELS.iter().copied().find(|(k, _, _)| *k == kind).ok_or_else(|| HttpError::not_found("Unknown agent"))
}

/// Cheap availability check: (available, detail, version, signedIn).
async fn status(kind: &str) -> (bool, String, Option<String>, Option<bool>) {
    match kind {
        CLAUDE => {
            let v = claude_version().await;
            let detail = v.as_ref().map(|v| format!("CLI {v}")).unwrap_or_else(|| not_found(&claude_bin()));
            (v.is_some(), detail, parse_version(v.as_deref()), v.as_ref().map(|_| claude_signed_in()))
        }
        CODEX => {
            let (acp, custom, installed) = codex_ready();
            let method = codex_auth();
            let detail = match (&installed, method) {
                (None, _) => "Not installed — see Harness".to_string(),
                (Some(_), Some(_)) => format!("codex-acp {}", acp.unwrap_or_default()),
                (Some(_), None) => "Not signed in — see Harness".to_string(),
            };
            let version = installed.clone().filter(|_| !custom);
            (installed.is_some() && method.is_some(), detail, version, installed.map(|_| method.is_some()))
        }
        OPENCODE => {
            let v = opencode_version().await;
            let detail = v.as_ref().map(|v| format!("CLI {v}")).unwrap_or_else(|| not_found(&opencode_bin()));
            (v.is_some(), detail, parse_version(v.as_deref()), None)
        }
        _ => {
            let v = grok_version().await;
            let signed = grok_logged_in();
            let detail = match &v {
                None => not_found(&grok_bin()),
                Some(v) if signed => format!("CLI {v}"),
                Some(_) => "Not signed in — see Harness".into(),
            };
            (v.is_some() && signed, detail, v.clone(), v.map(|_| signed))
        }
    }
}

async fn models_of(s: &AppState, kind: &str) -> Value {
    match kind {
        CLAUDE => {
            // the list the CLI shows in /model; "default" = no --model, so it isn't offered as a choice
            let options: Vec<Value> = claude_model_list(s, false).await.into_iter().filter(|m| m["id"] != "default").collect();
            json!({ "models": options.iter().map(|m| m["id"].clone()).collect::<Vec<_>>(), "options": options })
        }
        CODEX => json!({ "models": kv_models(s, CODEX) }),
        OPENCODE => json!({ "models": connected_models(None).await }),
        _ => json!({ "models": grok_models(s) }),
    }
}

async fn harness_of(s: &AppState, kind: &str, refresh: bool) -> Value {
    match kind {
        CLAUDE => claude_harness(s, refresh).await,
        CODEX => codex_harness(s).await,
        OPENCODE => opencode_harness().await,
        _ => grok_harness(s).await,
    }
}

/// The agents list with the time it was read.
type AgentsCache = Mutex<Option<(Instant, Vec<Value>)>>;

/// Harness caches: each harness for a minute, the agents list for a minute.
#[derive(Default, Clone)]
pub struct Caches {
    harness: Arc<Mutex<HashMap<String, (Instant, Value)>>>,
    agents: Arc<AgentsCache>,
}

impl Caches {
    /// After install, login, logout, update, settings — data is recomputed.
    pub fn invalidate(&self, kind: Option<&str>) {
        match kind {
            Some(k) => {
                self.harness.lock().expect("harness cache").remove(k);
                clear_latest(k);
            }
            None => self.harness.lock().expect("harness cache").clear(),
        }
        *self.agents.lock().expect("agents cache") = None;
    }
}

/// One harness: `{ info, checkedAt }`; `refresh` also re-checks the latest version online.
pub async fn harness_one(s: &AppState, kind: &str, refresh: bool) -> ApiResult<Value> {
    label_of(kind)?;
    let caches = &s.harness;
    if !refresh
        && let Some((at, v)) = caches.harness.lock().expect("harness cache").get(kind)
        && at.elapsed() < Duration::from_secs(60)
    {
        return Ok(v.clone());
    }
    if refresh {
        clear_latest(kind);
        *caches.agents.lock().expect("agents cache") = None;
    }
    let data = json!({ "info": harness_of(s, kind, refresh).await, "checkedAt": db::now() });
    caches.harness.lock().expect("harness cache").insert(kind.to_string(), (Instant::now(), data.clone()));
    Ok(data)
}

/// Agent availability for selection in the UI (faster than the full harness).
pub async fn agents_list(s: &AppState) -> Value {
    let cached = s
        .harness
        .agents
        .lock()
        .expect("agents cache")
        .as_ref()
        .filter(|(at, _)| at.elapsed() < Duration::from_secs(60))
        .map(|(_, v)| v.clone());
    let list = match cached {
        Some(l) => l,
        None => {
            let mut list = vec![];
            for (kind, label, style) in LABELS {
                let (available, detail, version, signed_in) = status(kind).await;
                list.push(json!({ "kind": kind, "label": label, "modelStyle": style, "available": available, "detail": detail, "version": version, "signedIn": signed_in }));
            }
            *s.harness.agents.lock().expect("agents cache") = Some((Instant::now(), list.clone()));
            list
        }
    };
    json!({ "agents": list, "defaultKind": CLAUDE })
}

// ---------- routes ----------

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/agents", get(agents_route))
        .route("/agents/{kind}/models", get(models_route))
        .route("/harness", get(all_route))
        .route("/harness/agents", get(agents_route))
        .route("/harness/{kind}", get(one_route))
        .route("/harness/{kind}/default-model", put(default_model_route))
        .route("/harness/{kind}/accounts/{provider}/{account}/{op}", post(account_route))
        .route("/harness/{kind}/actions/{action}", post(action_route))
}

#[derive(Deserialize)]
struct RefreshQuery {
    refresh: Option<String>,
}

async fn agents_route(State(s): State<AppState>) -> axum::Json<Value> {
    axum::Json(agents_list(&s).await)
}

async fn models_route(State(s): State<AppState>, UrlPath(kind): UrlPath<String>) -> ApiResult<axum::Json<Value>> {
    label_of(&kind)?;
    Ok(axum::Json(models_of(&s, &kind).await))
}

/// All harnesses at once (kept for API clients; the UI reads them per tab).
async fn all_route(State(s): State<AppState>, Query(q): Query<RefreshQuery>) -> ApiResult<axum::Json<Value>> {
    let refresh = q.refresh.as_deref() == Some("1");
    let mut all = vec![];
    for (kind, _, _) in LABELS {
        all.push(harness_one(&s, kind, refresh).await?["info"].clone());
    }
    Ok(axum::Json(json!({ "harness": all, "checkedAt": db::now() })))
}

async fn one_route(
    State(s): State<AppState>,
    UrlPath(kind): UrlPath<String>,
    Query(q): Query<RefreshQuery>,
) -> ApiResult<axum::Json<Value>> {
    Ok(axum::Json(harness_one(&s, &kind, q.refresh.as_deref() == Some("1")).await?))
}

#[derive(Deserialize)]
struct DefaultModelBody {
    model: Option<String>,
}

async fn default_model_route(
    State(s): State<AppState>,
    UrlPath(kind): UrlPath<String>,
    JsonBody(b): JsonBody<DefaultModelBody>,
) -> ApiResult<axum::Json<Value>> {
    label_of(&kind)?;
    if b.model.as_ref().is_some_and(|m| m.encode_utf16().count() > 200) {
        return Err(HttpError::bad_request("model: at most 200 characters"));
    }
    let model = b.model.as_deref().map(str::trim).filter(|m| !m.is_empty());
    match kind.as_str() {
        CLAUDE => set_claude_default_model(model)?,
        OPENCODE => set_opencode_default_model(model)?,
        _ => return Err(HttpError::not_found("This agent has no default model setting")),
    }
    s.harness.invalidate(Some(&kind));
    Ok(axum::Json(json!({ "ok": true })))
}

/// Run one of the harness's own actions (install, update, login…) as a terminal job. Only
/// actions from the harness's list run — never arbitrary commands.
async fn action_route(
    State(s): State<AppState>,
    UrlPath((kind, id)): UrlPath<(String, String)>,
    crate::http::LenientBody(size): crate::http::LenientBody<crate::runs::TermSize>,
) -> ApiResult<(axum::http::StatusCode, axum::Json<hubness_core::JobInfo>)> {
    let (_, label, _) = label_of(&kind)?;
    size.check()?;
    let info = harness_one(&s, &kind, false).await?;
    let act = info["info"]["actions"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["id"] == id.as_str()).cloned())
        .ok_or_else(|| HttpError::not_found("Action is not available"))?;
    if act["runnable"] != true {
        return Err(HttpError::new(403, "This command needs root — run it in a terminal yourself"));
    }
    let caches = s.harness.clone();
    let k = kind.clone();
    let job = crate::terminal::start_job(crate::terminal::JobOpts {
        title: format!("{label} · {id}"),
        command: act["command"].as_str().unwrap_or_default().to_string(),
        cols: size.cols,
        rows: size.rows,
        on_exit: Some(Box::new(move |_| caches.invalidate(Some(&k)))),
        ..Default::default()
    });
    Ok((axum::http::StatusCode::CREATED, axum::Json(job)))
}

/// Provider credentials (accountMode: providers): switch / sign out one connection.
async fn account_route(
    State(s): State<AppState>,
    UrlPath((kind, provider, account, op)): UrlPath<(String, String, String, String)>,
) -> ApiResult<axum::Json<Value>> {
    label_of(&kind)?;
    if kind != OPENCODE {
        return Err(HttpError::not_found("This agent has no provider accounts"));
    }
    let valid = |v: &str| (1..=120).contains(&v.len()) && v.chars().all(|c| c.is_ascii_alphanumeric() || "_.:-".contains(c));
    if !valid(&provider) || !valid(&account) {
        return Err(HttpError::bad_request("Bad account id"));
    }
    if op != "switch" && op != "logout" {
        return Err(HttpError::bad_request("op: switch | logout"));
    }
    let (code, output) = run_once(&opencode_bin(), &["auth", &op, &provider, &account]).await;
    s.harness.invalidate(Some(&kind));
    if code != Some(0) {
        return Err(HttpError::new(500, if output.is_empty() { format!("opencode auth {op} failed") } else { output }));
    }
    Ok(axum::Json(json!({ "ok": true, "output": output })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version(Some("2.1.332 (Claude Code)")).as_deref(), Some("2.1.332"));
        assert!(cmp_version("1.10.0", "1.9.9").is_gt());
        assert_eq!(version_info(Some("1.0.0"), Some("1.0.1"), "npm", None)["status"], "update-available");
        assert_eq!(version_info(None, None, "—", None)["status"], "not-installed");
    }

    #[test]
    fn quoting_and_tiers() {
        assert_eq!(shq("claude"), "claude");
        assert_eq!(shq("it's"), "'it'\\''s'");
        assert_eq!(human_tier(Some("default_claude_max_20x")).as_deref(), Some("Max 20x"));
        assert_eq!(human_tier(Some("default_pro_plan")).as_deref(), Some("pro plan"));
    }

    #[test]
    fn limits_merge_prefers_fresher_turn_numbers() {
        let cli = Some((
            vec![
                json!({ "id": "weekly_all", "utilization": 10, "resetsAt": null }),
                json!({ "id": "session", "group": "session", "scope": null, "utilization": 5, "resetsAt": null }),
            ],
            "2026-01-01T00:00:00.000Z".to_string(),
        ));
        let ours = Some((json!([{ "id": "seven_day", "utilization": 40, "resetsAt": "x" }]), "2026-01-02T00:00:00.000Z".to_string()));
        let (l, at) = merge_limits(cli, ours);
        assert_eq!(l[0]["utilization"], 40);
        assert_eq!(l[0]["resetsAt"], "x");
        assert_eq!(at.as_deref(), Some("2026-01-02T00:00:00.000Z"));
        assert_eq!(main_first(vec![json!({ "id": "a" }), json!({ "id": "seven_day" })])[0]["id"], "seven_day");
    }
}
