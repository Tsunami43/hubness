//! Kanban cards of a project and starting an agent from a card — `routes/kanban.ts` and
//! `db/cards.ts`.

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use hubness_core::{Card, CardColumn, GlobalStreamMsg, PermissionMode, Session};
use rusqlite::{Connection, OptionalExtension, Row};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::db::{new_id, now, sessions as repo};
use crate::error::{ApiResult, HttpError};
use crate::http::JsonBody;
use crate::{AppState, agents, catalog, runtime, workspaces};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/projects/{id}/cards", get(list).post(create))
        .route("/cards/{id}", patch(update).delete(remove))
        .route("/cards/{id}/start", post(start))
}

fn from_row(r: &Row) -> rusqlite::Result<Card> {
    Ok(Card {
        id: r.get("id")?,
        project_id: r.get("project_id")?,
        title: r.get("title")?,
        description: r.get("description")?,
        column: CardColumn::parse(&r.get::<_, String>("col")?),
        position: r.get("position")?,
        session_id: r.get("session_id")?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

fn get_card(db: &Connection, id: &str) -> rusqlite::Result<Option<Card>> {
    db.query_row("SELECT * FROM cards WHERE id = ?", [id], from_row).optional()
}

fn next_position(db: &Connection, project_id: &str, col: CardColumn) -> rusqlite::Result<f64> {
    let m: f64 =
        db.query_row("SELECT COALESCE(MAX(position), 0) FROM cards WHERE project_id = ? AND col = ?", (project_id, col.as_str()), |r| {
            r.get(0)
        })?;
    Ok(m + 1.0)
}

#[derive(Default)]
struct CardPatch {
    title: Option<String>,
    description: Option<String>,
    column: Option<CardColumn>,
    position: Option<f64>,
    session_id: Option<String>,
}

/// Update a card; moving it to another column without a position puts it last there.
fn update_card(db: &Connection, id: &str, p: CardPatch) -> rusqlite::Result<Option<Card>> {
    let Some(cur) = get_card(db, id)? else { return Ok(None) };
    let mut position = p.position;
    if let Some(col) = p.column
        && col != cur.column
        && position.is_none()
    {
        position = Some(next_position(db, &cur.project_id, col)?);
    }
    db.execute(
        "UPDATE cards SET title = ?, description = ?, col = ?, position = ?, session_id = ?, updated_at = ? WHERE id = ?",
        (
            p.title.unwrap_or(cur.title),
            p.description.unwrap_or(cur.description),
            p.column.unwrap_or(cur.column).as_str(),
            position.unwrap_or(cur.position),
            p.session_id.or(cur.session_id),
            now(),
            id,
        ),
    )?;
    get_card(db, id)
}

/// Mark the card of a finished turn as ready for review (called by the runtime).
pub fn card_to_review(db: &Connection, card_id: &str) -> rusqlite::Result<()> {
    if get_card(db, card_id)?.is_some_and(|c| c.column == CardColumn::Doing) {
        update_card(db, card_id, CardPatch { column: Some(CardColumn::Review), ..Default::default() })?;
    }
    Ok(())
}

async fn list(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<axum::Json<Vec<Card>>> {
    s.with(|db, _| {
        let mut st = db.prepare("SELECT * FROM cards WHERE project_id = ? ORDER BY col, position")?;
        Ok(st.query_map([&id], from_row)?.collect::<Result<_, _>>()?)
    })
}

fn too_long(v: &Option<String>, max: usize, field: &str) -> ApiResult<()> {
    if v.as_ref().is_some_and(|t| t.encode_utf16().count() > max) {
        return Err(HttpError::bad_request(format!("{field}: at most {max} characters")));
    }
    Ok(())
}

#[derive(Deserialize)]
struct CreateBody {
    title: String,
    #[serde(default)]
    description: String,
    column: Option<CardColumn>,
}

async fn create(
    State(s): State<AppState>,
    Path(id): Path<String>,
    JsonBody(b): JsonBody<CreateBody>,
) -> ApiResult<(StatusCode, axum::Json<Card>)> {
    let title = b.title.trim().to_string();
    if title.is_empty() {
        return Err(HttpError::bad_request("title: Title is required"));
    }
    too_long(&Some(title.clone()), 500, "title")?;
    too_long(&Some(b.description.clone()), 100_000, "description")?;
    let card = s.with(|db, _| {
        let p = catalog::project_or_404(db, &id)?;
        let col = b.column.unwrap_or(CardColumn::Todo);
        let cid = new_id();
        let t = now();
        db.execute(
            "INSERT INTO cards (id, project_id, title, description, col, position, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            (&cid, &p.id, &title, &b.description, col.as_str(), next_position(db, &p.id, col)?, &t, &t),
        )?;
        Ok(get_card(db, &cid)?.expect("card just inserted"))
    })?;
    Ok((StatusCode::CREATED, card))
}

#[derive(Deserialize)]
struct PatchBody {
    title: Option<String>,
    description: Option<String>,
    column: Option<CardColumn>,
    position: Option<f64>,
}

async fn update(State(s): State<AppState>, Path(id): Path<String>, JsonBody(b): JsonBody<PatchBody>) -> ApiResult<axum::Json<Card>> {
    too_long(&b.title, 500, "title")?;
    too_long(&b.description, 100_000, "description")?;
    s.with(|db, _| {
        let p = CardPatch {
            title: b.title.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(String::from),
            description: b.description,
            column: b.column,
            position: b.position,
            session_id: None,
        };
        update_card(db, &id, p)?.ok_or_else(|| HttpError::not_found("Card not found"))
    })
}

async fn remove(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<axum::Json<Value>> {
    s.with(|db, _| {
        db.execute("DELETE FROM cards WHERE id = ?", [&id])?;
        Ok(json!({ "ok": true }))
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentBody {
    kind: String,
    model: Option<String>,
    permission_mode: Option<PermissionMode>,
}

#[derive(Deserialize)]
struct StartBody {
    agent: AgentBody,
}

/// Start an agent from a card: a new project session, prompt = title + description.
async fn start(
    State(s): State<AppState>,
    c: Option<axum::Extension<crate::auth::Caller>>,
    Path(id): Path<String>,
    JsonBody(b): JsonBody<StartBody>,
) -> ApiResult<(StatusCode, axum::Json<Session>)> {
    let (session, prompt) = s
        .with(|db, cfg| {
            let card = get_card(db, &id)?.ok_or_else(|| HttpError::not_found("Card not found"))?;
            let project = catalog::project_or_404(db, &card.project_id)?;
            let mode = catalog::settings(db, cfg)?.default_permission;
            let agent = agents::validate(&b.agent.kind, b.agent.model.as_deref(), b.agent.permission_mode, mode)
                .ok_or_else(|| HttpError::bad_request("Agent is not specified"))?;
            crate::permissions::Access::of(c.as_deref()).agent(&agent.kind, agent.model.as_deref())?;
            let workspace = workspaces::project_workspace(db, &project)?;
            let session = repo::create(
                db,
                repo::New {
                    id: None,
                    project_id: Some(&project.id),
                    group_id: None,
                    workspace_id: Some(&workspace.id),
                    title: &card.title,
                    agent: &agent,
                    cwd: None,
                },
            )?;
            db.execute("UPDATE sessions SET card_id = ? WHERE id = ?", (&card.id, &session.id))?;
            crate::permissions::set_owner(db, &session.id, c.as_deref())?;
            update_card(
                db,
                &card.id,
                CardPatch { session_id: Some(session.id.clone()), column: Some(CardColumn::Doing), ..Default::default() },
            )?;
            let prompt =
                if card.description.trim().is_empty() { card.title.clone() } else { format!("{}\n\n{}", card.title, card.description) };
            Ok((repo::get(db, &session.id)?.expect("session exists"), prompt))
        })?
        .0;
    s.bus.emit_global(GlobalStreamMsg::Session { session: session.clone() });
    Ok((StatusCode::CREATED, axum::Json(runtime::send_message(&s, &session.id, &prompt, vec![])?)))
}
