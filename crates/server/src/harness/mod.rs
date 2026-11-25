//! Harness data of the registered agents — a port of `harness.ts`, `agents/kit.ts` and the
//! harness parts of each agent module (`agents/*/index.ts`, `claude/harness.ts`,
//! `claude/models.ts`): the light availability list for pickers, the full per-agent info
//! for the Harness page (versions, update, login, limits, models, default model), provider
//! accounts and default-model settings.
//!
//! Everything is read from official CLI commands and their local files; tokens are never
//! read. The "latest version" checks go to the npm registry, GitHub releases, pacman or the
//! agent's own updater, cached for an hour.

mod claude;
mod codex;
mod grok;
mod kit;
mod opencode;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{Path as UrlPath, Query, State};
use axum::routing::{get, post, put};
use futures_util::future::join_all;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::AppState;
use crate::db;
use crate::error::{ApiResult, HttpError};
use crate::http::JsonBody;

use claude::*;
use codex::*;
use grok::*;
use kit::*;
use opencode::*;

pub(crate) use kit::read_json;

// ---------- registry: status, models, harness ----------

/// Registered agents in UI order (the order of the TS registry).
pub(crate) const LABELS: &[(&str, &str, &str)] =
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
            // each check starts a CLI: run them side by side
            let list: Vec<Value> = join_all(LABELS.iter().map(|(kind, label, style)| async move {
                let (available, detail, version, signed_in) = status(kind).await;
                json!({ "kind": kind, "label": label, "modelStyle": style, "available": available, "detail": detail, "version": version, "signedIn": signed_in })
            }))
            .await;
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
    let all = join_all(LABELS.iter().map(|(kind, _, _)| harness_one(&s, kind, refresh)))
        .await
        .into_iter()
        .map(|h| h.map(|mut h| h["info"].take()))
        .collect::<ApiResult<Vec<_>>>()?;
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
    fn tiers() {
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
