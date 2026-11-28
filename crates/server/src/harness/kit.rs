//! Shared helpers of the harness modules: running CLIs, versions, the latest-version cache.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::agents::acp::models_key;
use crate::db::kv;
use crate::error::{ApiResult, HttpError};
use crate::{AppState, agents};

pub(super) use crate::terminal::shq;

/// Run a CLI with stdin closed; stdout on success, `None` on failure or timeout.
pub(super) async fn run_cli(cmd: &str, args: &[&str], timeout: Duration, env: &[(String, String)]) -> Option<String> {
    // a lazy mise wrapper would download the tool just to answer a status check: not installed
    if !cmd.contains('/') && agents::find_on_path(cmd).is_some_and(|r| r.is_err()) {
        return None;
    }
    let mut c = tokio::process::Command::new(cmd);
    c.args(args)
        .env_clear()
        .envs(crate::config::child_env())
        .envs(env.iter().cloned())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(timeout, c.output()).await.ok()?.ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run a command once: exit code and stdout+stderr without ANSI colors (the TS `runOnce`).
pub(super) async fn run_once(cmd: &str, args: &[&str]) -> (Option<i32>, String) {
    let child = tokio::process::Command::new(cmd)
        .args(args)
        .env_clear()
        .envs(crate::config::child_env())
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

pub(super) fn strip_ansi(s: &str) -> String {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\x1b\[[0-9;]*m").expect("valid")).replace_all(s, "").into_owned()
}

pub(crate) fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The first `x.y.z` in the text.
pub(super) fn parse_version(s: Option<&str>) -> Option<String> {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(\d+\.\d+\.\d+)").expect("valid")).captures(s?).map(|c| c[1].to_string())
}

pub(super) fn cmp_version(a: &str, b: &str) -> std::cmp::Ordering {
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

pub(super) fn version_info(installed: Option<&str>, latest: Option<&str>, source: &str, update: Option<&str>) -> Value {
    let status = match (installed, latest) {
        (None, _) => "not-installed",
        (Some(_), None) => "unknown",
        (Some(i), Some(l)) if cmp_version(i, l).is_lt() => "update-available",
        _ => "up-to-date",
    };
    json!({ "installed": installed, "latest": latest, "status": status, "source": source, "updateCommand": update })
}

pub(super) fn action(id: &str, command: &str, runnable: bool, interactive: bool) -> Value {
    json!({ "id": id, "command": command, "runnable": runnable, "interactive": interactive })
}

pub(super) fn act(id: &str, command: &str) -> Value {
    action(id, command, true, true)
}

pub(super) fn not_found(bin: &str) -> String {
    format!("Not installed (“{bin}” not found) — see Harness")
}

pub(super) type LatestCache = Mutex<HashMap<String, (Instant, Option<String>)>>;

pub(super) fn latest_cache() -> &'static LatestCache {
    static C: OnceLock<LatestCache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Latest versions from the network, per agent kind, cached for an hour (failures too).
pub(super) async fn cached_latest<F: std::future::Future<Output = Option<String>>>(
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
pub(super) fn clear_latest(kind: &str) {
    latest_cache().lock().expect("latest cache").retain(|k, _| !k.starts_with(&format!("{kind}\0")));
}

pub(super) fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| reqwest::Client::builder().timeout(Duration::from_secs(10)).user_agent("hubness").build().expect("http client"))
}

pub(super) async fn npm_latest(pkg: &str) -> Option<String> {
    let res = http().get(format!("https://registry.npmjs.org/{pkg}/latest")).send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    let v: Value = res.json().await.ok()?;
    parse_version(v["version"].as_str())
}

/// Full path of a binary found on PATH (a real install, never a lazy mise wrapper), with
/// symlinks resolved.
pub(super) fn resolve_bin(bin: &str) -> Option<PathBuf> {
    let found = if bin.contains('/') { Some(PathBuf::from(bin)).filter(|p| p.is_file()) } else { agents::find_on_path(bin)?.ok() }?;
    Some(std::fs::canonicalize(&found).unwrap_or(found))
}

pub(super) use crate::paths::home;

pub(super) fn env_set(k: &str) -> bool {
    std::env::var(k).is_ok_and(|v| !v.is_empty())
}

/// Models an ACP agent declared in its last session (stored in kv).
pub(super) fn kv_models(s: &AppState, kind: &str) -> Vec<String> {
    s.db().ok().and_then(|db| kv::get::<Vec<String>>(&db, &models_key(kind)).ok().flatten()).unwrap_or_default()
}

pub(super) fn write_json_key(path: &Path, key: &str, value: Option<&str>, init: Option<Value>, label: &str) -> ApiResult<()> {
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
