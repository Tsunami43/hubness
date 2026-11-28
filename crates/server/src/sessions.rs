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
use crate::{AppState, agents, catalog, inbox, runtime, workspaces};

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
        .route("/sessions/{id}/approvals/{request_id}", post(approval))
        .route(
            "/sessions/{id}/attachments",
            post(upload).layer(axum::extract::DefaultBodyLimit::max(crate::attachments::MAX_ATTACHMENT + 1024)),
        )
        .route("/sessions/{id}/attachments/{file}", get(attachment))
        .route("/sessions/{id}/project-suggestion", get(project_suggestion))
        .route("/sessions/{id}/convert", post(convert))
        .route("/sessions/{id}/file", get(pane_file))
        .route("/sessions/{id}/diff", get(pane_diff))
        .route("/sessions/{id}/pane", post(set_pane))
        .route("/sessions/{id}/browser", post(browser_route))
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
    if b.prompt.as_deref().is_some_and(|p| p.encode_utf16().count() > 1_000_000) {
        return Err(HttpError::bad_request("prompt: at most 1000000 characters"));
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
    match b.prompt.as_deref().filter(|p| !p.trim().is_empty()) {
        Some(prompt) => Ok((StatusCode::CREATED, axum::Json(runtime::send_message(&s, &session.0.id, prompt, vec![])?))),
        None => Ok((StatusCode::CREATED, session)),
    }
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

/// The session's page in the agents' browser goes with it (in the background: the caller may
/// hold the database).
fn close_browser_page(id: &str) {
    let id = id.to_string();
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        rt.spawn(async move { crate::browser::close_session_browser(&id).await });
    }
}

/// `?folder=1` (one session) / `?folders=1` (the archive): deleting a regular chat also
/// removes its folder (what the agent made there).
#[derive(Deserialize, Default)]
struct DeleteQuery {
    folder: Option<String>,
    folders: Option<String>,
}

fn flag(v: &Option<String>) -> bool {
    matches!(v.as_deref(), Some("1" | "true"))
}

/// Delete one session for good: stop its agent, drop it from the lists, remove its attached
/// files and — when asked — the folder of a regular chat. Files go after the database: a file
/// that can't be removed never keeps a session alive.
fn delete_now(s: &AppState, db: &Connection, id: &str, folder: bool) -> ApiResult<()> {
    let cur = repo::get(db, id)?;
    s.runtime.stop(id);
    close_browser_page(id);
    // where the files are, read before the session and its events are gone
    let mut cwd = None;
    let mut legacy = vec![];
    let mut chat_dir = None;
    if let Some(cur) = &cur {
        // a regular chat's folder is not created again just to look into it
        cwd = if cur.project_id.is_some() || cur.group_id.is_some() {
            runtime::session_cwd(db, s.config(), cur).ok().map(std::path::PathBuf::from)
        } else {
            Some(cur.cwd.as_ref().map(std::path::PathBuf::from).unwrap_or_else(|| s.config().chats_dir.join(&cur.id)))
        };
        legacy = crate::attachments::ids_in_events(&repo::events(db, id)?);
        if folder {
            chat_dir = inbox::chat_folder_of(db, s.config(), cur)?;
        }
    }
    repo::delete(db, id)?;
    if let Some(dir) = chat_dir {
        let _ = std::fs::remove_dir_all(&dir).or_else(|_| std::fs::remove_file(&dir));
    } else if let Some(cwd) = cwd {
        crate::attachments::remove_session(&cwd, id, &legacy);
    }
    s.bus.emit_global(GlobalStreamMsg::SessionDeleted { id: id.to_string() });
    Ok(())
}

async fn remove(State(s): State<AppState>, Path(id): Path<String>, Query(q): Query<DeleteQuery>) -> ApiResult<axum::Json<Value>> {
    s.with(|db, _| {
        delete_now(&s, db, &id, flag(&q.folder))?;
        Ok(json!({ "ok": true }))
    })
}

/// Empty the archive (the client confirms first); `?folders=1` takes regular chats' folders along.
async fn empty_archive(State(s): State<AppState>, Query(q): Query<DeleteQuery>) -> ApiResult<axum::Json<Value>> {
    s.with(|db, _| {
        let list = repo::list(db, Filter { archived: Archived::Only, ..Default::default() })?;
        for x in &list {
            delete_now(&s, db, &x.id, flag(&q.folders))?;
        }
        Ok(json!({ "ok": true, "deleted": list.len() }))
    })
}

async fn set_archived(s: AppState, id: String, archived: bool) -> ApiResult<axum::Json<Session>> {
    let session = s.with(|db, _| {
        let cur = session_or_404(db, &id)?;
        if archived && s.runtime.is_active(&cur.id) {
            return Err(HttpError::new(409, "The agent is still working — wait for it or stop it"));
        }
        repo::set_archived(db, &cur.id, archived)?.ok_or_else(|| HttpError::not_found("Session not found"))
    })?;
    if archived {
        close_browser_page(&id);
    }
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

#[derive(Deserialize)]
struct MessageBody {
    #[serde(default)]
    text: String,
    attachments: Option<Value>,
}

async fn messages(State(s): State<AppState>, Path(id): Path<String>, JsonBody(b): JsonBody<MessageBody>) -> ApiResult<axum::Json<Session>> {
    if b.text.encode_utf16().count() > 1_000_000 {
        return Err(HttpError::bad_request("text: at most 1000000 characters"));
    }
    let files = {
        let db = s.db()?;
        let cur = session_or_404(&db, &id)?;
        let cwd = runtime::session_cwd(&db, s.config(), &cur)?;
        let files = crate::attachments::resolve(std::path::Path::new(&cwd), &cur.id, b.attachments.as_ref())?;
        // writing to an archived session brings it back
        if cur.archived_at.is_some() && (!b.text.trim().is_empty() || !files.is_empty()) {
            repo::set_archived(&db, &cur.id, false)?;
        }
        files
    };
    Ok(axum::Json(runtime::send_message(&s, &id, &b.text, files)?))
}

async fn stop(State(s): State<AppState>, Path(id): Path<String>) -> axum::Json<Value> {
    axum::Json(json!({ "ok": s.runtime.stop(&id) }))
}

async fn approval(
    State(s): State<AppState>,
    Path((id, request_id)): Path<(String, String)>,
    JsonBody(b): JsonBody<agents::ApprovalDecision>,
) -> ApiResult<axum::Json<Value>> {
    if b.reason.as_ref().is_some_and(|r| r.encode_utf16().count() > 10_000) {
        return Err(HttpError::bad_request("reason: at most 10000 characters"));
    }
    if let Some(answers) = &b.answers
        && answers.values().any(|v| !v.is_string())
    {
        return Err(HttpError::bad_request("answers: values must be strings"));
    }
    if !s.runtime.respond(&id, &request_id, b) {
        return Err(HttpError::not_found("The request is no longer pending"));
    }
    Ok(axum::Json(json!({ "ok": true })))
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

pub(crate) fn truncate(s: &str, max: usize) -> String {
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

// ---------- the agent's headless browser (MCP preview_* tools) ----------

async fn browser_route(
    State(s): State<AppState>,
    Path(id): Path<String>,
    JsonBody(b): JsonBody<crate::browser::BrowserRequest>,
) -> ApiResult<axum::Json<hubness_core::BrowserResult>> {
    let project = {
        let db = s.db()?;
        let session = session_or_404(&db, &id)?;
        session.project_id.and_then(|pid| crate::catalog::project_or_404(&db, &pid).ok())
    };
    // http://localhost:<port> of a project session goes through the project's preview
    let preview_for: Option<crate::browser::PreviewFor> = project.map(|p| {
        let state = s.clone();
        Box::new(move |port: u16| {
            let db = state.db().ok()?;
            crate::runs::preview_port_for(&db, state.config(), &p, port).ok()
        }) as crate::browser::PreviewFor
    });
    Ok(axum::Json(crate::browser::browser_action(s.config(), &id, b, preview_for).await?))
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

// ---------- attachments ----------

/// Attach a file to the next message: the raw bytes in the body, the name in X-Filename
/// (URI-encoded), the type in Content-Type. Saved into the session's working folder.
async fn upload(
    State(s): State<AppState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> ApiResult<(StatusCode, axum::Json<hubness_core::Attachment>)> {
    let cwd = {
        let db = s.db()?;
        let cur = session_or_404(&db, &id)?;
        runtime::session_cwd(&db, s.config(), &cur)?
    };
    let header = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    let raw_name = header("x-filename");
    let name = percent_decode(if raw_name.is_empty() { "file" } else { &raw_name }).unwrap_or_else(|| "file".into());
    let mime = header("content-type");
    let a = crate::http::blocking(move || crate::attachments::save(std::path::Path::new(&cwd), &id, &name, &mime, &body)).await?;
    Ok((StatusCode::CREATED, a))
}

/// `decodeURIComponent`; `None` on a malformed sequence.
fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// An attachment's content (thumbnails and downloads in the feed). Attached HTML/SVG is
/// never rendered as a page of the hub's origin.
async fn attachment(State(s): State<AppState>, Path((id, file)): Path<(String, String)>) -> ApiResult<axum::response::Response> {
    use axum::http::header;
    use axum::response::IntoResponse;
    let path = {
        let db = s.db()?;
        let cur = session_or_404(&db, &id)?;
        let cwd = runtime::session_cwd(&db, s.config(), &cur)?;
        crate::attachments::path_of(std::path::Path::new(&cwd), &cur.id, &file)?
    };
    let mime = crate::attachments::serve_mime(&path);
    let disposition = if mime.starts_with("image/") || mime.starts_with("video/") { "inline" } else { "attachment" };
    let bytes = tokio::fs::read(&path).await?;
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (header::CONTENT_DISPOSITION, disposition),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CONTENT_SECURITY_POLICY, "default-src 'none'; img-src 'self'; media-src 'self'; style-src 'unsafe-inline'"),
        ],
        bytes,
    )
        .into_response())
}

// ---------- Inbox → Project ----------

async fn project_suggestion(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<axum::Json<hubness_core::ProjectSuggestion>> {
    s.with(|db, _| Ok(inbox::suggestion(&session_or_404(db, &id)?)))
}

#[derive(Deserialize)]
struct ConvertBody {
    #[serde(default)]
    name: String,
}

/// Move a chat from Inbox into a new project and continue the same session there.
async fn convert(State(s): State<AppState>, Path(id): Path<String>, JsonBody(b): JsonBody<ConvertBody>) -> ApiResult<axum::Json<Value>> {
    if b.name.encode_utf16().count() > 200 {
        return Err(HttpError::bad_request("name: at most 200 characters"));
    }
    if s.runtime.is_active(&id) {
        return Err(HttpError::new(409, "The agent is still working — wait for it or stop it"));
    }
    let (project, session) = s
        .with(|db, cfg| {
            let cur = session_or_404(db, &id)?;
            let Some(from) = cur.cwd.clone().filter(|_| cur.project_id.is_none()) else {
                return Err(HttpError::bad_request("Only inbox chats can be converted"));
            };
            let name = if b.name.trim().is_empty() { cur.title.clone() } else { b.name.trim().to_string() };
            let dest = cfg.projects_dir.join(inbox::slug(&name));
            if dest.exists() {
                return Err(HttpError::new(409, format!("Folder already exists: {}", dest.display())));
            }
            if std::fs::rename(&from, &dest).is_err() {
                copy_dir(std::path::Path::new(&from), &dest)?;
                std::fs::remove_dir_all(&from)?;
            }
            if !dest.join(".git").exists() {
                let _ = std::process::Command::new("git").args(["init", "-q"]).arg(&dest).status();
            }
            if !dest.join("AGENTS.md").exists() {
                std::fs::write(dest.join("AGENTS.md"), format!("# {name}\n\nProject rules for any agent working here.\n"))?;
            }
            let dest_s = crate::paths::to_string(&dest);
            let project = crate::db::projects::create(db, &name, &dest_s, hubness_core::ProjectKind::Normal)?;
            crate::manifest::create(&project)?;
            db.execute(
                "UPDATE sessions SET project_id = ?, cwd = NULL, updated_at = ? WHERE id = ?",
                (&project.id, crate::db::now(), &cur.id),
            )?;
            // continue the same session if the agent can carry its transcript over to the new path,
            // otherwise pass the conversation summary as the next message
            if !relocate(db, &cur, &from, &dest_s) {
                let handoff = inbox::handoff_text(&repo::events(db, &cur.id)?, &from, &dest_s);
                db.execute(
                    "UPDATE sessions SET agent_session_id = NULL, handoff = ?, updated_at = ? WHERE id = ?",
                    (handoff, crate::db::now(), &cur.id),
                )?;
            }
            let project = crate::db::projects::get(db, &project.id)?.expect("project exists");
            Ok((project, repo::get(db, &cur.id)?.expect("session exists")))
        })?
        .0;
    announce(&s, &session);
    Ok(axum::Json(json!({ "project": project, "session": session })))
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        let t = e.file_type()?;
        if t.is_dir() {
            copy_dir(&e.path(), &target)?;
        } else if t.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(e.path())?, &target)?;
        } else {
            std::fs::copy(e.path(), &target)?;
        }
    }
    Ok(())
}

/// The session folder moved: Claude Code continues the same session if its transcript is
/// copied into the folder of the new path (other agents get a summary instead).
fn relocate(db: &Connection, s: &Session, from: &str, to: &str) -> bool {
    if s.agent.kind != "claude-code" {
        return false;
    }
    let Some(sid) = &s.agent_session_id else { return false };
    let config_dir = match s.agent.account.as_deref().filter(|a| *a != "default") {
        Some(account) => crate::db::kv::get::<Vec<Value>>(db, "claude.profiles")
            .ok()
            .flatten()
            .and_then(|ps| ps.into_iter().find(|p| p["id"] == account))
            .and_then(|p| p["dir"].as_str().map(std::path::PathBuf::from)),
        None => None,
    }
    .unwrap_or_else(|| std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".claude"));
    let project_dir = |cwd: &str| {
        config_dir.join("projects").join(cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect::<String>())
    };
    let src = project_dir(from).join(format!("{sid}.jsonl"));
    if !src.exists() {
        return false;
    }
    let dst = project_dir(to);
    std::fs::create_dir_all(&dst).is_ok() && std::fs::copy(&src, dst.join(format!("{sid}.jsonl"))).is_ok()
}

// ---------- files for the pane ----------

/// Folders a pane file may come from: the project, the group's repositories and workspace,
/// or the chat folder.
fn pane_roots(db: &Connection, cfg: &crate::Config, s: &Session) -> ApiResult<Vec<String>> {
    if let Some(pid) = &s.project_id {
        return Ok(crate::db::projects::get(db, pid)?.map(|p| vec![p.path]).unwrap_or_default());
    }
    if let Some(gid) = &s.group_id {
        let Some(g) = crate::db::groups::get(db, gid)? else { return Ok(vec![]) };
        let mut roots: Vec<String> = catalog::group_projects(db, &g)?.into_iter().map(|p| p.path).collect();
        roots.push(runtime::session_cwd(db, cfg, s)?);
        return Ok(roots);
    }
    Ok(vec![runtime::session_cwd(db, cfg, s)?])
}

/// The file for the pane: relative to the session folder, or absolute inside one of its roots.
fn resolve_pane_file(s: &AppState, id: &str, target: &str) -> ApiResult<(String, String)> {
    let db = s.db()?;
    let session = session_or_404(&db, id)?;
    for root in pane_roots(&db, s.config(), &session)? {
        let rel =
            if target.starts_with('/') { target.strip_prefix(&format!("{root}/")).map(String::from) } else { Some(target.to_string()) };
        if let Some(rel) = rel {
            return Ok((root, rel));
        }
    }
    Err(HttpError::bad_request("File is outside the session folders"))
}

#[derive(Deserialize)]
struct PaneQuery {
    path: Option<String>,
}

async fn pane_file(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<PaneQuery>,
) -> ApiResult<axum::Json<hubness_core::FileContent>> {
    let (root, rel) = resolve_pane_file(&s, &id, q.path.as_deref().unwrap_or(""))?;
    crate::http::blocking(move || crate::files::read_file_safe(std::path::Path::new(&root), &rel)).await
}

async fn pane_diff(State(s): State<AppState>, Path(id): Path<String>, Query(q): Query<PaneQuery>) -> ApiResult<axum::Json<Value>> {
    let (root, rel) = resolve_pane_file(&s, &id, q.path.as_deref().unwrap_or(""))?;
    let root = std::path::Path::new(&root);
    crate::files::safe_join(root, &rel)?;
    let st = crate::git::status(root).await;
    let f = st.files.iter().find(|x| x.path == rel);
    let untracked = f.is_some_and(|f| f.untracked);
    let diff = crate::git::diff(root, Some(&rel), false, untracked).await;
    let staged = match f {
        Some(f) if f.index != " " && !f.untracked => crate::git::diff(root, Some(&rel), true, false).await,
        _ => String::new(),
    };
    let parts: Vec<String> = [staged, diff].into_iter().filter(|d| !d.is_empty()).collect();
    Ok(axum::Json(json!({ "diff": parts.join("\n") })))
}
