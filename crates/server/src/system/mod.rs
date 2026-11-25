//! `/api/system`: server monitoring (a port of `routes/system.ts` and `system/*`);
//! `POST /api/ask`: “Ask about this”.

pub mod ask;
pub mod exec;
pub mod host;
pub mod monitor;
pub mod packages;

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use hubness_core::{Session, SystemOverview, SystemPlatform, UnitScope};
use serde_json::{Value, json};

use crate::AppState;
use crate::error::{ApiResult, HttpError};
use crate::http::JsonBody;
use monitor::{ActionResult, lines_param};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/system/platform", get(platform))
        .route("/system/overview", get(overview))
        .route("/system/containers", get(containers))
        .route("/system/containers/{name}", axum::routing::delete(container_remove))
        // `logs` is a GET; a POST there is an unknown action, as in the TS router
        .route(
            "/system/containers/{name}/logs",
            get(container_logs).post(|Path(name): Path<String>| container_action(Path((name, "logs".into())))),
        )
        .route("/system/containers/{name}/{action}", post(container_action))
        .route("/system/networks", get(networks))
        .route("/system/networks/{name}", axum::routing::delete(network_remove))
        .route("/system/volumes", get(volumes))
        .route("/system/volumes/{name}", axum::routing::delete(volume_remove))
        .route("/system/units", get(units))
        .route(
            "/system/units/{scope}/{name}/logs",
            get(unit_logs).post(|Path((scope, name)): Path<(String, String)>| unit_action(Path((scope, name, "logs".into())))),
        )
        .route("/system/units/{scope}/{name}/{action}", post(unit_action))
        .route("/system/packages", get(|| async { Json(json!({ "managers": packages::package_managers() })) }))
        .route("/system/packages/{manager}", get(manager_packages))
        .route("/system/package-info/{name}", get(package_info))
        .route("/ask", post(handle_ask))
}

type Q = Query<HashMap<String, String>>;

const ACTIONS: &[&str] = &["start", "stop", "restart"];

fn action(a: &str) -> ApiResult<&str> {
    if ACTIONS.contains(&a) { Ok(a) } else { Err(HttpError::bad_request("action: start | stop | restart")) }
}

fn scope(s: &str) -> ApiResult<UnitScope> {
    match s {
        "system" => Ok(UnitScope::System),
        "user" => Ok(UnitScope::User),
        _ => Err(HttpError::bad_request("scope: system | user")),
    }
}

async fn platform() -> Json<SystemPlatform> {
    Json(host::system_platform())
}

async fn overview() -> Json<SystemOverview> {
    Json(monitor::overview().await)
}

async fn containers() -> ApiResult<Json<Value>> {
    let list = monitor::docker_containers().await?;
    Ok(Json(json!({ "available": list.is_some(), "containers": list.unwrap_or_default() })))
}

async fn container_remove(Path(name): Path<String>) -> ApiResult<Json<ActionResult>> {
    Ok(Json(monitor::container_remove(&name).await?))
}

async fn networks() -> ApiResult<Json<Value>> {
    let list = monitor::docker_networks().await?;
    Ok(Json(json!({ "available": list.is_some(), "networks": list.unwrap_or_default() })))
}

async fn network_remove(Path(name): Path<String>) -> ApiResult<Json<ActionResult>> {
    Ok(Json(monitor::network_remove(&name).await?))
}

async fn volumes() -> ApiResult<Json<Value>> {
    let list = monitor::docker_volumes().await?;
    Ok(Json(json!({ "available": list.is_some(), "volumes": list.unwrap_or_default() })))
}

async fn volume_remove(Path(name): Path<String>) -> ApiResult<Json<ActionResult>> {
    Ok(Json(monitor::volume_remove(&name).await?))
}

async fn container_action(Path((name, a)): Path<(String, String)>) -> ApiResult<Json<ActionResult>> {
    let a = action(&a)?;
    Ok(Json(monitor::container_action(&name, a).await))
}

async fn container_logs(Path(name): Path<String>, Query(q): Q) -> Json<Value> {
    let logs = monitor::container_logs(&name, lines_param(q.get("lines").map(String::as_str))).await;
    Json(json!({ "logs": logs }))
}

async fn units() -> ApiResult<Json<Value>> {
    Ok(Json(json!(monitor::units().await?)))
}

async fn unit_action(Path((sc, name, a)): Path<(String, String, String)>) -> ApiResult<Json<ActionResult>> {
    let sc = scope(&sc)?;
    let a = action(&a)?;
    Ok(Json(monitor::unit_action(&name, sc, a).await))
}

async fn unit_logs(Path((sc, name)): Path<(String, String)>, Query(q): Q) -> ApiResult<Json<Value>> {
    let sc = scope(&sc)?;
    let logs = monitor::unit_logs(&name, sc, lines_param(q.get("lines").map(String::as_str))).await;
    Ok(Json(json!({ "logs": logs })))
}

async fn manager_packages(Path(id): Path<String>, Query(q): Q) -> ApiResult<Json<hubness_core::PackagesResponse>> {
    Ok(Json(packages::manager_packages(&id, q.get("refresh").map(String::as_str) == Some("1")).await?))
}

async fn package_info(Path(name): Path<String>) -> Json<Value> {
    Json(json!({ "info": monitor::package_info(&name).await }))
}

/// “Ask about this”: an agent session with the object's context attached.
async fn handle_ask(State(s): State<AppState>, JsonBody(body): JsonBody<Value>) -> ApiResult<(StatusCode, Json<Session>)> {
    let req = ask::parse_request(&body)?;
    Ok((StatusCode::CREATED, Json(ask::ask_about(&s, req).await?)))
}
