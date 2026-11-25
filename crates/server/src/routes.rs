//! `/api` routes ported so far. Request bodies are checked at the boundary (`JsonBody`);
//! the catalog keeps its own checks with specific messages.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use hubness_core::PermissionMode;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::AppState;
use crate::catalog::{self, GroupFields, ProjectPatch};
use crate::db::groups;
use crate::error::{ApiResult, HttpError};
use crate::http::{JsonBody, double_option};

pub fn api() -> Router<AppState> {
    Router::new()
        .route("/health", get(|| async { Json(json!({ "ok": true })) }))
        .route("/projects", get(list_projects))
        .route("/projects/discover", get(discover))
        .route("/projects/import", axum::routing::post(import))
        .route("/projects/{id}", axum::routing::patch(update_project).delete(delete_project))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/groups", get(list_groups).post(create_group))
        .route("/groups/{id}", get(group_detail).patch(update_group).delete(delete_group))
        .merge(crate::sessions::routes())
        .fallback(|| async { HttpError::not_found("Not found") })
}

type Q = Query<HashMap<String, String>>;

// ---------- projects ----------

async fn list_projects(State(s): State<AppState>) -> ApiResult<Json<Value>> {
    s.with(|db, _| Ok(json!(catalog::list_projects(db)?)))
}

async fn discover(State(s): State<AppState>, Query(q): Q) -> ApiResult<Json<Value>> {
    s.with(|db, cfg| Ok(json!(catalog::discover_repos(db, cfg, q.get("path").map(String::as_str))?)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportBody {
    paths: Vec<String>,
    group_id: Option<String>,
}

async fn import(State(s): State<AppState>, JsonBody(b): JsonBody<ImportBody>) -> ApiResult<(StatusCode, Json<Value>)> {
    let list = s.with(|db, _| Ok(json!(catalog::import_projects(db, &b.paths, b.group_id.as_deref())?)))?;
    Ok((StatusCode::CREATED, list))
}

#[derive(Deserialize)]
struct ProjectPatchBody {
    name: Option<String>,
    description: Option<String>,
    instructions: Option<String>,
}

async fn update_project(
    State(s): State<AppState>,
    Path(id): Path<String>,
    JsonBody(b): JsonBody<ProjectPatchBody>,
) -> ApiResult<Json<Value>> {
    let patch = ProjectPatch { name: b.name, description: b.description, instructions: b.instructions };
    s.with(|db, _| Ok(json!(catalog::update_project(db, &id, patch)?)))
}

/// Remove a project from the hub. Files stay on disk.
async fn delete_project(State(s): State<AppState>, Path(id): Path<String>, Query(q): Q) -> ApiResult<Json<Value>> {
    if q.get("files").map(String::as_str) == Some("1") {
        // TODO(rust-port): port deletableFolder() and its safety checks before enabling this
        return Err(HttpError::new(501, "Deleting project files is not supported by the Rust server yet"));
    }
    s.with(|db, cfg| {
        catalog::remove_project(db, cfg, &id)?;
        Ok(json!({ "ok": true, "deleted": null }))
    })
}

// ---------- settings ----------

async fn get_settings(State(s): State<AppState>) -> ApiResult<Json<Value>> {
    s.with(|db, cfg| Ok(json!(catalog::settings(db, cfg)?)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsBody {
    default_permission: Option<PermissionMode>,
    default_instructions: Option<String>,
    session_instructions: Option<String>,
}

async fn put_settings(State(s): State<AppState>, JsonBody(b): JsonBody<SettingsBody>) -> ApiResult<Json<Value>> {
    if b.default_permission.is_none() && b.default_instructions.is_none() && b.session_instructions.is_none() {
        return Err(HttpError::bad_request("Nothing to save"));
    }
    s.with(|db, cfg| {
        if let Some(m) = b.default_permission {
            catalog::set_default_permission(db, m)?;
        }
        if let Some(t) = &b.default_instructions {
            catalog::set_default_instructions(db, t)?;
        }
        if let Some(t) = &b.session_instructions {
            catalog::set_session_instructions(db, t)?;
        }
        Ok(json!(catalog::settings(db, cfg)?))
    })
}

// ---------- project groups ----------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GroupBody {
    name: Option<String>,
    description: Option<String>,
    instructions: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    folder: Option<Option<String>>,
    project_ids: Option<Vec<String>>,
}

impl From<GroupBody> for GroupFields {
    fn from(b: GroupBody) -> Self {
        Self { name: b.name, description: b.description, instructions: b.instructions, folder: b.folder, project_ids: b.project_ids }
    }
}

async fn list_groups(State(s): State<AppState>) -> ApiResult<Json<Value>> {
    s.with(|db, _| Ok(json!(groups::list(db)?)))
}

async fn group_detail(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    s.with(|db, cfg| Ok(json!(catalog::group_detail(db, cfg, &id)?)))
}

async fn create_group(State(s): State<AppState>, JsonBody(b): JsonBody<GroupBody>) -> ApiResult<(StatusCode, Json<Value>)> {
    let g = s.with(|db, cfg| Ok(json!(catalog::create_group(db, cfg, b.into())?)))?;
    Ok((StatusCode::CREATED, g))
}

async fn update_group(State(s): State<AppState>, Path(id): Path<String>, JsonBody(b): JsonBody<GroupBody>) -> ApiResult<Json<Value>> {
    s.with(|db, cfg| Ok(json!(catalog::update_group(db, cfg, &id, b.into())?)))
}

async fn delete_group(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    s.with(|db, cfg| {
        catalog::delete_group(db, cfg, &id)?;
        Ok(json!({ "ok": true }))
    })
}
