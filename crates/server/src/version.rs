//! `GET /api/system/version`, `POST /api/system/update` (a port of `version.ts`): the hub's
//! version, the latest release and a self-update from the web.
//!
//! The latest version is `<base>/latest/download/VERSION` of the GitHub Releases that
//! `hubness update` installs from (`HUBNESS_RELEASE_URL` replaces the base, also with a
//! `file://` folder; `HUBNESS_NO_UPDATE_CHECK=1` turns the check off).
//!
//! The update runs `hubness update --yes` from the installed binary (registered by the
//! `hubness` command with [`set_updater`]); it replaces the binary and restarts the systemd
//! service, which restarts this server with the new version. A hub run in the foreground
//! keeps running the old version until it is restarted (`installed`).

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::Query;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use hubness_core::version::{Version, compare};
use hubness_core::{HubUpdateStage, HubUpdateState, HubVersion};
use serde_json::{Value, json};

use crate::AppState;
use crate::error::{ApiResult, HttpError};

pub const HUB_VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASES: &str = "https://github.com/Tsunami43/hubness/releases";
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// A failed check is repeated sooner than a successful one.
const RETRY: Duration = Duration::from_secs(10 * 60);

static UPDATER: OnceLock<PathBuf> = OnceLock::new();
static UPDATE: Mutex<HubUpdateState> = Mutex::new(HubUpdateState { state: HubUpdateStage::Idle, message: None });

struct Cached {
    at: Instant,
    base: String,
    latest: Option<String>,
}
static CACHE: tokio::sync::Mutex<Option<Cached>> = tokio::sync::Mutex::const_new(None);

/// The installed `hubness` binary that can replace itself (`hubness update`). Without it
/// (tests, a library user) the hub can't update itself.
pub fn set_updater(exe: PathBuf) {
    let _ = UPDATER.set(exe);
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/system/version", get(version)).route("/system/update", post(update))
}

fn releases_url() -> String {
    std::env::var("HUBNESS_RELEASE_URL").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| RELEASES.into()).trim_end_matches('/').to_string()
}

fn check_disabled() -> bool {
    std::env::var("HUBNESS_NO_UPDATE_CHECK").is_ok_and(|v| !v.is_empty() && v != "0")
}

async fn fetch_latest(base: &str) -> Option<String> {
    let url = format!("{base}/latest/download/VERSION");
    let body = if let Some(path) = url.strip_prefix("file://") {
        tokio::fs::read_to_string(path).await.ok()?
    } else {
        let client =
            reqwest::Client::builder().timeout(Duration::from_secs(5)).user_agent(format!("hubness/{HUB_VERSION}")).build().ok()?;
        let resp = client.get(&url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.text().await.ok()?
    };
    let v = body.trim().trim_start_matches('v').to_string();
    Version::parse(&v).map(|_| v)
}

/// The latest released version (cached for hours; `force` checks again).
pub async fn latest_version(force: bool) -> Option<String> {
    if check_disabled() {
        return None;
    }
    let base = releases_url();
    // held across the request: concurrent callers wait for one check
    let mut cache = CACHE.lock().await;
    if let Some(c) = cache.as_ref().filter(|c| c.base == base)
        && !force
        && c.at.elapsed() < if c.latest.is_some() { CHECK_EVERY } else { RETRY }
    {
        return c.latest.clone();
    }
    let fetched = fetch_latest(&base).await;
    // offline: keep the last known version
    let latest = fetched.or_else(|| cache.as_ref().filter(|c| c.base == base).and_then(|c| c.latest.clone()));
    *cache = Some(Cached { at: Instant::now(), base, latest: latest.clone() });
    latest
}

fn update_state() -> HubUpdateState {
    UPDATE.lock().unwrap().clone()
}

async fn version(Query(q): Query<std::collections::HashMap<String, String>>) -> Json<HubVersion> {
    let latest = latest_version(q.get("check").is_some_and(|v| v == "1")).await;
    let base = releases_url();
    Json(HubVersion {
        version: HUB_VERSION.into(),
        update_available: latest.as_deref().is_some_and(|l| compare(l, HUB_VERSION).is_gt()),
        release_url: latest
            .as_ref()
            .filter(|_| base.starts_with("http://") || base.starts_with("https://"))
            .map(|l| format!("{base}/tag/v{l}")),
        latest,
        can_update: UPDATER.get().is_some(),
        update: update_state(),
    })
}

fn set_state(state: HubUpdateStage, message: Option<String>) {
    *UPDATE.lock().unwrap() = HubUpdateState { state, message };
}

/// The last lines of a command's output (an error message for the web).
fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.trim().lines().collect();
    let tail = lines[lines.len().saturating_sub(6)..].join("\n");
    let chars: Vec<char> = tail.chars().collect();
    chars[chars.len().saturating_sub(2000)..].iter().collect()
}

async fn update() -> ApiResult<(StatusCode, Json<Value>)> {
    let Some(exe) = UPDATER.get().cloned() else {
        return Err(HttpError::bad_request("This hub can't update itself: it doesn't run from an installed hubness binary"));
    };
    {
        let mut st = UPDATE.lock().unwrap();
        if st.state == HubUpdateStage::Running {
            return Err(HttpError::new(409, "An update is already running"));
        }
        *st = HubUpdateState { state: HubUpdateStage::Running, message: None };
    }
    tokio::spawn(async move {
        // the hub's own binary, not an agent: it needs the hub settings (HUBNESS_RELEASE_URL, …)
        let out = tokio::process::Command::new(&exe)
            .args(["update", "--yes"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await;
        // on success under systemd the service restarts and this process is gone by now
        match out {
            Ok(o) if o.status.success() => set_state(HubUpdateStage::Installed, Some(tail(&String::from_utf8_lossy(&o.stdout)))),
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                let msg = if err.trim().is_empty() { String::from_utf8_lossy(&o.stdout).to_string() } else { err.to_string() };
                set_state(HubUpdateStage::Failed, Some(tail(&msg)));
            }
            Err(e) => set_state(HubUpdateStage::Failed, Some(format!("can't run {}: {e}", exe.display()))),
        }
    });
    Ok((StatusCode::ACCEPTED, Json(json!({ "ok": true }))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_the_last_lines() {
        let text = (1..=10).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
        assert_eq!(tail(&text), "5\n6\n7\n8\n9\n10");
        assert_eq!(tail("  one\n"), "one");
    }
}
