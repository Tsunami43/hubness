//! `/api` routes ported so far. Request bodies are checked at the boundary (`JsonBody`);
//! the catalog keeps its own checks with specific messages.

use std::collections::{HashMap, HashSet};

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
        // clone by URL is a terminal job (/api/jobs/:id/stream); the project appears after a successful clone
        .route("/projects/clone", axum::routing::post(clone_project))
        .route("/projects/{id}", get(project_detail).patch(update_project).delete(delete_project))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/groups", get(list_groups).post(create_group))
        .route("/groups/{id}", get(group_detail).patch(update_group).delete(delete_group))
        .route("/groups/{id}/context", get(group_context))
        .merge(crate::auth::routes())
        .merge(crate::sessions::routes())
        .merge(crate::kanban::routes())
        .merge(crate::secrets::routes())
        .merge(crate::manifest::routes())
        .merge(crate::system::routes())
        .merge(crate::version::routes())
        .merge(crate::files::routes())
        .merge(crate::git::routes())
        .merge(crate::extensions::routes())
        .merge(crate::harness::routes())
        .merge(crate::git_auth::routes())
        .merge(crate::terminal::routes())
        .merge(crate::runs::routes())
        .merge(crate::environments::routes())
        .merge(crate::voice::routes())
        .merge(crate::ssh::routes())
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
    let known: HashSet<String> = s.with(|db, _| Ok(crate::db::projects::list(db)?.into_iter().map(|p| p.path).collect()))?.0;
    let projects_dir = s.config().projects_dir.clone();
    crate::http::blocking(move || Ok(json!(catalog::discover_repos(&known, &projects_dir, q.get("path").map(String::as_str))?))).await
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloneBody {
    url: String,
    name: Option<String>,
    group_id: Option<String>,
    #[serde(flatten)]
    size: crate::runs::TermSize,
}

async fn clone_project(
    State(s): State<AppState>,
    crate::http::JsonBody(b): crate::http::JsonBody<CloneBody>,
) -> ApiResult<(axum::http::StatusCode, Json<hubness_core::JobInfo>)> {
    b.size.check()?;
    if b.url.encode_utf16().count() > 2000 {
        return Err(HttpError::bad_request("url: at most 2000 characters"));
    }
    if b.name.as_ref().is_some_and(|n| n.encode_utf16().count() > 200) {
        return Err(HttpError::bad_request("name: at most 200 characters"));
    }
    let job = catalog::clone_repo(&s, &b.url, b.name.as_deref(), b.group_id.as_deref(), &b.size)?;
    Ok((axum::http::StatusCode::CREATED, Json(job)))
}

/// Remove a project from the hub; its processes and container are stopped. Files stay on
/// disk unless `?files=1&confirm=<folder name>`: then the repository folder is deleted too
/// (irreversible; the folder name guards against an accidental call).
async fn delete_project(State(s): State<AppState>, Path(id): Path<String>, Query(q): Q) -> ApiResult<Json<Value>> {
    let with_files = q.get("files").map(String::as_str) == Some("1");
    let is_active = |sid: &str| s.runtime.is_active(sid);
    let dir = s
        .with(|db, cfg| {
            let p = catalog::assert_removable(db, &id, &is_active)?;
            let dir = if with_files { catalog::deletable_folder(db, cfg, &p)? } else { None };
            if let Some(d) = &dir
                && q.get("confirm").map(String::as_str) != Some(crate::paths::basename(d).as_str())
            {
                return Err(HttpError::bad_request("confirm must be the name of the project folder"));
            }
            // stop its Run commands, shells and previews
            crate::runs::stop_project_processes(db, &id)?;
            Ok(dir)
        })?
        .0;
    // the container is removed off the async threads
    let pid = id.clone();
    let _ = tokio::task::spawn_blocking(move || crate::environments::stop_container(&pid)).await;
    // a repository folder can be large: delete it off the async threads, without the database
    if let Some(d) = dir.clone() {
        let _ = crate::http::blocking(move || {
            std::fs::remove_dir_all(&d).map_err(|e| HttpError::new(500, format!("Could not delete {}: {e}", d.display())))
        })
        .await?;
    }
    s.with(|db, cfg| {
        catalog::remove_project(db, cfg, &id, &is_active)?;
        Ok(json!({ "ok": true, "deleted": dir.as_ref().map(|d| crate::paths::to_string(d)) }))
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
