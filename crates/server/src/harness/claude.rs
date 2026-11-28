//! Claude Code: version, sign-in, models (the CLI's /model list), limits, default model.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio::io::{AsyncWriteExt, BufReader};

use crate::db::kv;
use crate::error::{ApiResult, HttpError};
use crate::{AppState, agents};

use super::kit::*;

pub(super) const CLAUDE: &str = "claude-code";

pub(super) fn claude_bin() -> String {
    agents::claude::claude_bin()
}

pub(super) fn claude_json() -> PathBuf {
    home().join(".claude.json")
}

pub(super) fn claude_settings() -> PathBuf {
    home().join(".claude").join("settings.json")
}

pub(super) async fn claude_version() -> Option<String> {
    run_cli(&claude_bin(), &["--version"], Duration::from_secs(10), &[]).await.map(|v| v.trim().to_string())
}

/// Cheap check for pickers: the CLI keeps the signed-in account in ~/.claude.json.
pub(super) fn claude_signed_in() -> bool {
    read_json(&claude_json()).is_some_and(|j| j.get("oauthAccount").is_some_and(|o| !o.is_null() && o != &json!(false)))
        || env_set("ANTHROPIC_API_KEY")
}

pub(super) const MODELS_KEY: &str = "claude.models";
pub(super) const MODELS_TTL: Duration = Duration::from_secs(6 * 3600);

pub(super) fn fallback_models() -> Vec<Value> {
    vec![json!({ "id": "opus", "label": "Opus" }), json!({ "id": "sonnet", "label": "Sonnet" }), json!({ "id": "haiku", "label": "Haiku" })]
}

/// Models as the CLI lists them (the /model picker): the `initialize` control request of
/// stream-json mode returns `models`. No prompt is sent, so no tokens are spent.
pub(super) async fn read_claude_models() -> Option<Vec<Value>> {
    let mut child = tokio::process::Command::new(claude_bin())
        .args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"])
        .current_dir(std::env::temp_dir())
        .env_clear()
        .envs(crate::config::child_env())
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
pub(super) async fn claude_model_list(s: &AppState, refresh: bool) -> Vec<Value> {
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

pub(super) fn kv_with_time(db: &rusqlite::Connection, key: &str) -> Option<(Value, String)> {
    db.query_row("SELECT value, updated_at FROM kv WHERE key = ?", [key], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .ok()
        .and_then(|(v, t)| serde_json::from_str(&v).ok().map(|v| (v, t)))
}

pub(super) fn fresh(iso: &str, ttl: Duration) -> bool {
    iso.parse::<jiff::Timestamp>().is_ok_and(|t| (jiff::Timestamp::now().as_millisecond() - t.as_millisecond()) < ttl.as_millis() as i64)
}

pub(super) fn human_tier(tier: Option<&str>) -> Option<String> {
    let tier = tier.filter(|t| !t.is_empty())?;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    if let Some(c) = RE.get_or_init(|| regex::Regex::new(r"max_(\d+x)").expect("valid")).captures(tier) {
        return Some(format!("Max {}", &c[1]));
    }
    Some(tier.strip_prefix("default_").unwrap_or(tier).replace('_', " "))
}

pub(super) async fn claude_auth() -> Option<Value> {
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
pub(super) fn cli_cached_limits(claude_json: Option<&Value>) -> Option<(Vec<Value>, String)> {
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
    let at = crate::db::iso(jiff::Timestamp::from_millisecond(c["fetchedAtMs"].as_i64().unwrap_or(0)).ok()?);
    (!limits.is_empty()).then_some((limits, at))
}

/// Our rate_limit_event windows → the CLI limit they update.
pub(super) fn window_matches(ours: &str, l: &Value) -> Option<bool> {
    match ours {
        "seven_day" => Some(l["id"] == "seven_day" || l["id"] == "weekly_all"),
        "five_hour" => Some(l["id"] == "five_hour" || (l["group"] == "session" && (l["scope"].is_null() || l["scope"] == ""))),
        _ => None,
    }
}

/// The CLI list (all windows) with fresher numbers from our own turns where they match.
pub(super) fn merge_limits(cli: Option<(Vec<Value>, String)>, ours: Option<(Value, String)>) -> (Vec<Value>, Option<String>) {
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
pub(super) fn main_first(mut limits: Vec<Value>) -> Vec<Value> {
    if let Some(i) = limits.iter().position(|w| w["id"] == "weekly_all" || w["id"] == "seven_day")
        && i > 0
    {
        let w = limits.remove(i);
        limits.insert(0, w);
    }
    limits
}

pub(super) fn claude_default_model() -> Option<String> {
    read_json(&claude_settings()).and_then(|j| j["model"].as_str().filter(|m| !m.is_empty()).map(String::from))
}

/// Set / clear `model` in ~/.claude/settings.json; the rest of the file is untouched. Written
/// atomically through the real path (a symlinked settings.json stays a symlink).
pub(super) fn set_claude_default_model(model: Option<&str>) -> ApiResult<()> {
    if let Some(m) = model
        && !(m.len() <= 100 && !m.is_empty() && m.chars().all(|c| c.is_ascii_alphanumeric() || "_.:[]-".contains(c)))
    {
        return Err(HttpError::bad_request("Bad model id"));
    }
    write_json_key(&claude_settings(), "model", model, None, "settings.json")
}

/// Update command by install method: the binary's npm prefix or its own updater.
pub(super) fn claude_update_command(real: Option<&Path>) -> String {
    let real = real.map(crate::paths::to_string).unwrap_or_default();
    match real.find("/lib/node_modules/@anthropic-ai/claude-code/") {
        Some(i) => format!("npm install -g --prefix {} @anthropic-ai/claude-code@latest", shq(&real[..i])),
        None => format!("{} update", shq(&claude_bin())),
    }
}

pub(super) async fn claude_harness(s: &AppState, refresh: bool) -> Value {
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
