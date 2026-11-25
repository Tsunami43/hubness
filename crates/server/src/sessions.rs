//! Session routes (a port of `routes/sessions.ts` without the agent runtime): CRUD, archive,
//! the event feed, the pane next to the chat and the live SSE streams.

use std::convert::Infallible;
use std::time::Duration;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::routing::{get, post};
use futures_util::stream::{self, Stream, StreamExt};
use hubness_core::{AgentConfig, GlobalStreamMsg, Pane, PaneKind, PermissionMode, Session, SessionStreamMsg};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_stream::wrappers::{BroadcastStream, IntervalStream};

use crate::db::sessions::{self as repo, Archived, Filter};
use crate::db::{kv, now};
use crate::error::{ApiResult, HttpError};
use crate::http::{JsonBody, double_option};
use crate::{AppState, agents, catalog, inbox, workspaces};

/// Keep-alive period of the SSE streams (as in the TS server).
const PING: Duration = Duration::from_secs(25);

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sessions", get(list).post(create))
        .route("/sessions/archive", axum::routing::delete(empty_archive))
        .route("/sessions/{id}", get(detail).patch(patch).delete(remove))
        .route("/sessions/{id}/archive", post(archive))
        .route("/sessions/{id}/restore", post(restore))
        .route("/sessions/{id}/events", get(events))
        .route("/sessions/{id}/messages", post(messages))
        .route("/sessions/{id}/stop", post(stop))
        .route("/sessions/{id}/pane", post(set_pane))
        .route("/sessions/{id}/stream", get(session_stream))
        .route("/stream", get(global_stream))
}

fn session_or_404(db: &Connection, id: &str) -> ApiResult<Session> {
    repo::get(db, id)?.ok_or_else(|| HttpError::not_found("Session not found"))
}

/// Tell the session's stream and the lists that the session changed.
fn announce(s: &AppState, session: &Session) {
    s.bus.emit_session(&session.id, SessionStreamMsg::Session { session: session.clone() });
    s.bus.emit_global(GlobalStreamMsg::Session { session: session.clone() });
}

fn pane_key(id: &str) -> String {
    format!("pane:{id}")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListQuery {
    project_id: Option<String>,
    group_id: Option<String>,
    archived: Option<String>,
    all: Option<String>,
}

async fn list(State(s): State<AppState>, Query(q): Query<ListQuery>) -> ApiResult<axum::Json<Vec<Session>>> {
    s.with(|db, _| {
        let f = if let Some(p) = q.project_id.as_deref().filter(|v| !v.is_empty()) {
            Filter { project_id: Some(p), ..Default::default() }
        } else if let Some(g) = q.group_id.as_deref().filter(|v| !v.is_empty()) {
            Filter { group_id: Some(g), ..Default::default() }
        } else if q.archived.as_deref() == Some("1") {
            // the archive of every scope
            Filter { archived: Archived::Only, ..Default::default() }
        } else if q.all.as_deref() == Some("1") {
            Filter::default()
        } else {
            Filter { chats: true, ..Default::default() }
        };
        Ok(repo::list(db, f)?)
    })
}

// ---------- create / patch ----------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentBody {
    kind: Option<String>,
    model: Option<String>,
    permission_mode: Option<PermissionMode>,
    account: Option<String>,
}

impl AgentBody {
    fn check(&self) -> ApiResult<()> {
        let too_long = |v: &Option<String>, max: usize| v.as_ref().is_some_and(|s| s.encode_utf16().count() > max);
        if self.kind.as_ref().is_some_and(|k| k.is_empty()) || too_long(&self.kind, 64) {
            return Err(HttpError::bad_request("agent.kind: must be 1-64 characters"));
        }
        if too_long(&self.model, 200) {
            return Err(HttpError::bad_request("agent.model: at most 200 characters"));
        }
        if too_long(&self.account, 120) {
            return Err(HttpError::bad_request("agent.account: at most 120 characters"));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateBody {
    project_id: Option<String>,
    group_id: Option<String>,
    title: Option<String>,
    agent: AgentBody,
    prompt: Option<String>,
}

fn check_title(title: &Option<String>) -> ApiResult<()> {
    if title.as_ref().is_some_and(|t| t.encode_utf16().count() > 500) {
        return Err(HttpError::bad_request("title: at most 500 characters"));
    }
    Ok(())
}

fn validate_agent(db: &Connection, cfg: &crate::Config, a: &AgentBody) -> ApiResult<AgentConfig> {
    let kind = a.kind.as_deref().unwrap_or_default();
    let default_mode = catalog::settings(db, cfg)?.default_permission;
    agents::validate(kind, a.model.as_deref(), a.permission_mode, default_mode)
        .ok_or_else(|| HttpError::bad_request("Agent is not specified"))
}

async fn create(State(s): State<AppState>, JsonBody(b): JsonBody<CreateBody>) -> ApiResult<(StatusCode, axum::Json<Session>)> {
    check_title(&b.title)?;
    b.agent.check()?;
    if b.agent.kind.is_none() {
        return Err(HttpError::bad_request("agent.kind: required"));
    }
    let project_id = b.project_id.filter(|v| !v.is_empty());
    let group_id = b.group_id.filter(|v| !v.is_empty());
    // a session has exactly one scope: project, group, or regular chat
    if project_id.is_some() && group_id.is_some() {
        return Err(HttpError::bad_request("A session belongs to a project or a group, not both"));
    }
    if b.prompt.as_deref().is_some_and(|p| !p.trim().is_empty()) {
        // TODO(rust-port): start the first turn once the agent runtime is ported
        return Err(HttpError::new(501, "Starting a session with a prompt is not supported by the Rust server yet"));
    }
    let session = s.with(|db, cfg| {
        let workspace_id = if let Some(pid) = &project_id {
            Some(workspaces::project_workspace(db, &catalog::project_or_404(db, pid)?)?.id)
        } else if let Some(gid) = &group_id {
            let g = catalog::group_or_404(db, gid)?;
            Some(workspaces::group_workspace(db, cfg, &g.id, &catalog::group_projects(db, &g)?)?.id)
        } else {
            None
        };
        let agent = validate_agent(db, cfg, &b.agent)?;
        let id = crate::db::new_id();
        let scoped = project_id.is_some() || group_id.is_some();
        // regular chat: its own folder in Inbox (~/hub/inbox/<date>_<id>)
        let cwd = if scoped { None } else { Some(inbox::inbox_dir_for(cfg, &id)?) };
        let title = b.title.as_deref().map(str::trim).filter(|t| !t.is_empty()).unwrap_or(if scoped { "New session" } else { "New chat" });
        Ok(repo::create(
            db,
            repo::New {
                id: Some(id),
                project_id: project_id.as_deref(),
                group_id: group_id.as_deref(),
                workspace_id: workspace_id.as_deref(),
                title,
                agent: &agent,
                cwd: cwd.as_deref(),
            },
        )?)
    })?;
    s.bus.emit_global(GlobalStreamMsg::Session { session: session.0.clone() });
    Ok((StatusCode::CREATED, session))
}

async fn detail(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<axum::Json<Session>> {
    s.with(|db, _| session_or_404(db, &id))
}

#[derive(Deserialize)]
struct PatchBody {
    title: Option<String>,
    agent: Option<AgentBody>,
}

async fn patch(State(s): State<AppState>, Path(id): Path<String>, JsonBody(b): JsonBody<PatchBody>) -> ApiResult<axum::Json<Session>> {
    check_title(&b.title)?;
    if let Some(a) = &b.agent {
        a.check()?;
    }
    let session = s.with(|db, cfg| {
        let cur = session_or_404(db, &id)?;
        // the agent kind never changes within a session (the conversation lives with that agent);
        // the account stays: old sessions resume where their transcript lives
        let agent = match &b.agent {
            Some(a) => {
                let merged = AgentBody {
                    kind: Some(cur.agent.kind.clone()),
                    model: a.model.clone().or(cur.agent.model.clone()),
                    permission_mode: a.permission_mode.or(cur.agent.permission_mode),
                    account: None,
                };
                let mut next = validate_agent(db, cfg, &merged)?;
                next.account = cur.agent.account.clone();
                Some(next)
            }
            None => None,
        };
        let title = b.title.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(String::from);
        repo::update(db, &cur.id, repo::Patch { title, agent })?.ok_or_else(|| HttpError::not_found("Session not found"))
    })?;
    announce(&s, &session);
    Ok(session)
}

// ---------- delete / archive ----------

/// Delete one session for good and drop it from the lists.
fn delete_now(s: &AppState, db: &Connection, id: &str) -> ApiResult<()> {
    // TODO(rust-port): stop the session's agent first (needs the runtime)
    repo::delete(db, id)?;
    s.bus.emit_global(GlobalStreamMsg::SessionDeleted { id: id.to_string() });
    Ok(())
}

async fn remove(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<axum::Json<Value>> {
    s.with(|db, _| {
        delete_now(&s, db, &id)?;
        Ok(json!({ "ok": true }))
    })
}

/// Empty the archive (the client confirms first).
async fn empty_archive(State(s): State<AppState>) -> ApiResult<axum::Json<Value>> {
    s.with(|db, _| {
        let list = repo::list(db, Filter { archived: Archived::Only, ..Default::default() })?;
        for x in &list {
            delete_now(&s, db, &x.id)?;
        }
        Ok(json!({ "ok": true, "deleted": list.len() }))
    })
}

async fn set_archived(s: AppState, id: String, archived: bool) -> ApiResult<axum::Json<Session>> {
    // TODO(rust-port): refuse with 409 while the agent works (needs the runtime)
    let session = s.with(|db, _| {
        let cur = session_or_404(db, &id)?;
        repo::set_archived(db, &cur.id, archived)?.ok_or_else(|| HttpError::not_found("Session not found"))
    })?;
    announce(&s, &session);
    Ok(session)
}

async fn archive(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<axum::Json<Session>> {
    set_archived(s, id, true).await
}

async fn restore(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<axum::Json<Session>> {
    set_archived(s, id, false).await
}

// ---------- feed, turns ----------

async fn events(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<axum::Json<Vec<Value>>> {
    s.with(|db, _| Ok(repo::events(db, &id)?))
}

async fn messages(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<axum::Json<Value>> {
    session_or_404(&*s.db()?, &id)?;
    // TODO(rust-port): agent runtime
    Err(HttpError::new(501, "Sending messages is not supported by the Rust server yet"))
}

/// Stop the session's agent. Nothing runs in the Rust server yet, so there is nothing to stop.
async fn stop() -> axum::Json<Value> {
    axum::Json(json!({ "ok": false }))
}

// ---------- pane next to the chat (MCP open_pane) ----------

#[derive(Deserialize)]
struct PaneBody {
    #[serde(default, deserialize_with = "double_option")]
    pane: Option<Option<PaneInput>>,
}

#[derive(Deserialize)]
struct PaneInput {
    kind: PaneKind,
    target: Value,
    title: Option<String>,
}

fn truncate(s: &str, max: usize) -> String {
    // like JavaScript's slice: counted in UTF-16 units, never splitting a character
    let mut units = 0;
    s.chars()
        .take_while(|c| {
            units += c.len_utf16();
            units <= max
        })
        .collect()
}

async fn set_pane(State(s): State<AppState>, Path(id): Path<String>, JsonBody(b): JsonBody<PaneBody>) -> ApiResult<axum::Json<Value>> {
    let Some(input) = b.pane else { return Err(HttpError::bad_request("pane: required")) };
    let pane = input.map(|p| {
        let target = match p.target {
            Value::String(t) => t,
            other => other.to_string(),
        };
        Pane { kind: p.kind, target: truncate(&target, 2000), title: p.title.map(|t| truncate(&t, 200)), at: now() }
    });
    {
        let db = s.db()?;
        session_or_404(&db, &id)?;
        kv::set(&db, &pane_key(&id), &pane)?;
    }
    s.bus.emit_session(&id, SessionStreamMsg::Pane { pane });
    Ok(axum::Json(json!({ "ok": true })))
}

// ---------- live streams ----------

fn sse_event<T: serde::Serialize>(msg: &T) -> Result<Event, Infallible> {
    Ok(Event::default().data(serde_json::to_string(msg).unwrap_or_default()))
}

fn ticks() -> IntervalStream {
    IntervalStream::new(tokio::time::interval_at(tokio::time::Instant::now() + PING, PING))
}

/// The session's live stream: a snapshot first, then updates. The keep-alive carries the
/// current session, so a missed status change heals itself.
async fn session_stream(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let (first, rx, session) = {
        let db = s.db()?;
        let session = session_or_404(&db, &id)?;
        // subscribe while holding the connection: nothing written meanwhile is missed
        let rx = s.bus.on_session(&id);
        let pane = kv::get::<Option<Pane>>(&db, &pane_key(&id))?.flatten();
        let first = SessionStreamMsg::Snapshot { session: session.clone(), events: repo::events(&db, &id)?, pane };
        (first, rx, session)
    };
    let updates = BroadcastStream::new(rx).filter_map(|m| async move { m.ok().map(|m| sse_event(&m)) });
    let state = s.clone();
    let pings = ticks().map(move |_| {
        let current = state.db().ok().and_then(|db| repo::get(&db, &session.id).ok().flatten()).unwrap_or_else(|| session.clone());
        sse_event(&SessionStreamMsg::Session { session: current })
    });
    let body = stream::once(async move { sse_event(&first) }).chain(stream::select(updates, pings));
    Ok(Sse::new(body))
}

/// Session changes for the lists.
async fn global_stream(State(s): State<AppState>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let updates = BroadcastStream::new(s.bus.on_global()).filter_map(|m| async move { m.ok().map(|m| sse_event(&m)) });
    let pings = ticks().map(|_| Ok(Event::default().event("ping").data("")));
    Sse::new(stream::select(updates, pings))
}
