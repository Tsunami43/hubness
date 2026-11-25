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
        .route("/projects/new-repo", axum::routing::post(new_repo))
        // clone by URL is a terminal job: not ported yet
        .route("/projects/clone", axum::routing::post(|| async { HttpError::not_found("Not found") }))
        .route("/projects/{id}", get(project_detail).patch(update_project).delete(delete_project))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/groups", get(list_groups).post(create_group))
        .route("/groups/{id}", get(group_detail).patch(update_group).delete(delete_group))
        .route("/groups/{id}/context", get(group_context))
        .merge(crate::sessions::routes())
        .merge(crate::kanban::routes())
        .merge(crate::secrets::routes())
        .merge(crate::manifest::routes())
        .merge(crate::files::routes())
        .merge(crate::git::routes())
        .merge(crate::git_auth::routes())
        .fallback(|| async { HttpError::not_found("Not found") })
}

type Q = Query<HashMap<String, String>>;

// ---------- projects ----------

async fn list_projects(State(s): State<AppState>) -> ApiResult<Json<Vec<hubness_core::Project>>> {
    let list = s.with(|db, _| catalog::list_projects(db))?;
    catalog::refresh_repo_meta_in_background(&s, &list.0);
    Ok(list)
}

async fn project_detail(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<hubness_core::ProjectDetail>> {
    Ok(Json(catalog::project_detail(&s, &id).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewRepoBody {
    name: Option<String>,
    group_id: Option<String>,
}

/// A new empty project (folder + git init) in the group folder or the projects folder.
async fn new_repo(State(s): State<AppState>, JsonBody(b): JsonBody<NewRepoBody>) -> ApiResult<(StatusCode, Json<hubness_core::Project>)> {
    if b.name.as_ref().is_some_and(|n| n.encode_utf16().count() > 200) {
        return Err(HttpError::bad_request("name: Too big: expected string to have <=200 characters"));
    }
    let p = catalog::create_repo(&s, b.name.as_deref(), b.group_id.as_deref()).await?;
    Ok((StatusCode::CREATED, Json(p)))
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
        catalog::remove_project(db, cfg, &id, &|sid| s.runtime.is_active(sid))?;
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

/// Aggregated context the agent will receive in a group session (preview).
async fn group_context(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let g = s.with(|db, _| catalog::group_or_404(db, &id))?.0;
    catalog::refresh_group_repos(&s, &g).await?;
    s.with(|db, _| {
        let g = catalog::group_or_404(db, &g.id)?;
        Ok(json!({ "context": catalog::group_agent_context(db, &g)? }))
    })
}

async fn delete_group(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    s.with(|db, cfg| {
        catalog::delete_group(db, cfg, &id, &|sid| s.runtime.is_active(sid))?;
        Ok(json!({ "ok": true }))
    })
}
