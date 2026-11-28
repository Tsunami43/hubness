//! Project manifest `.hub/hub.yaml` — a port of `manifest.ts`: shared memory of the human
//! and the agent (links, SSH hosts, containers, units, documents). The UI builds the
//! Overview tab from it; the agent receives it as context.

use std::fs;
use std::path::PathBuf;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use hubness_core::{Manifest, ManifestResponse, Project, ServiceState};
use serde_json::{Map, Value};

use crate::error::ApiResult;
use crate::{AppState, catalog, paths};

pub fn manifest_path(p: &Project) -> PathBuf {
    std::path::Path::new(&p.path).join(".hub").join("hub.yaml")
}

/// JavaScript's `String(v)` for a YAML value.
fn js_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        Value::Array(a) => a.iter().map(js_string).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".into(),
        other => other.to_string(),
    }
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a.iter().map(js_string).filter(|s| !s.is_empty()).collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => vec![],
    }
}

fn normalize(raw: &Value) -> Manifest {
    let mut links = Map::new();
    if let Some(Value::Object(l)) = raw.get("links") {
        for (k, v) in l {
            links.insert(k.clone(), Value::String(js_string(v)));
        }
    }
    Manifest {
        links,
        ssh: str_list(raw.get("ssh")),
        services: str_list(raw.get("services")),
        units: str_list(raw.get("units")),
        docs: str_list(raw.get("docs")),
        notes: raw.get("notes").and_then(Value::as_str).map(String::from),
    }
}

/// The manifest, if present and valid. `Err` — the file exists but does not parse.
pub fn read(p: &Project) -> (bool, Result<Option<Manifest>, String>) {
    let path = manifest_path(p);
    if !path.exists() {
        return (false, Ok(None));
    }
    let parsed = fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|text| serde_norway::from_str::<Option<Value>>(&text).map_err(|e| e.to_string()))
        .map(|v| Some(normalize(&v.unwrap_or(Value::Object(Map::new())))));
    (true, parsed)
}

const TEMPLATE: &str = "# hubness project manifest — shared memory for you and the agents.
# The hub builds the Overview tab from it; agents get it as context.
links:
  # grafana: http://localhost:3000/d/{name}
  # staging: http://localhost:3001
ssh: []        # hosts from ~/.ssh/config, e.g. [prod, vps-proxy]
services: []   # docker containers of this project
units: []      # systemd units of this project
docs:
  - AGENTS.md
notes: |
  Anything agents should always know about this project.
";

/// Create the manifest from the template (an existing one is left alone).
pub fn create(p: &Project) -> std::io::Result<PathBuf> {
    let path = manifest_path(p);
    if path.exists() {
        return Ok(path);
    }
    fs::create_dir_all(path.parent().expect("has a parent"))?;
    let name: String =
        p.name.chars().map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '-' }).collect::<String>().to_lowercase();
    fs::write(&path, TEMPLATE.replace("{name}", &name))?;
    Ok(path)
}

/// Short manifest text for the agent's system prompt (non-empty sections only).
pub fn prompt(p: &Project) -> Option<String> {
    let (_, Ok(Some(m))) = read(p) else { return None };
    let full = serde_json::to_value(&m).ok()?;
    let compact: Map<String, Value> = full
        .as_object()?
        .iter()
        .filter(|(_, v)| match v {
            Value::Array(a) => !a.is_empty(),
            Value::Object(o) => !o.is_empty(),
            Value::String(s) => !s.is_empty(),
            _ => false,
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if compact.is_empty() {
        return None;
    }
    let yaml = serde_norway::to_string(&compact).ok()?;
    Some(format!(
        "This project has a hub manifest (.hub/hub.yaml). Use it to resolve references like \"check prod grafana\" or \"restart the db\":\n{yaml}"
    ))
}

/// Docker containers by name: (state, status). `None` — Docker is not available.
async fn docker_containers() -> Option<Vec<(String, String, String)>> {
    let out = tokio::process::Command::new("docker")
        .args(["ps", "-a", "--format", "{{json .}}"])
        .env_clear()
        .envs(crate::config::child_env())
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .map(|r| (js_string(&r["Names"]), js_string(&r["State"]), js_string(&r["Status"])))
            .collect(),
    )
}

async fn response(p: &Project) -> ManifestResponse {
    let (exists, parsed) = read(p);
    let (manifest, error) = match parsed {
        Ok(m) => (m, None),
        Err(e) => (None, Some(e)),
    };
    let mut services = vec![];
    if let Some(m) = manifest.as_ref().filter(|m| !m.services.is_empty()) {
        let containers = docker_containers().await.unwrap_or_default();
        services = m
            .services
            .iter()
            .map(|name| match containers.iter().find(|c| &c.0 == name) {
                Some(c) => ServiceState { name: name.clone(), state: c.1.clone(), status: c.2.clone() },
                None => ServiceState { name: name.clone(), state: "missing".into(), status: "not found".into() },
            })
            .collect();
    }
    ManifestResponse { exists, path: paths::to_string(&manifest_path(p)), manifest, error, services }
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/projects/{id}/manifest", get(get_manifest).post(create_manifest))
}

async fn get_manifest(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<axum::Json<ManifestResponse>> {
    let p = s.with(|db, _| catalog::project_or_404(db, &id))?.0;
    Ok(axum::Json(response(&p).await))
}

async fn create_manifest(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<(StatusCode, axum::Json<ManifestResponse>)> {
    let p = s.with(|db, _| catalog::project_or_404(db, &id))?.0;
    create(&p)?;
    Ok((StatusCode::CREATED, axum::Json(response(&p).await)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalizes_like_the_ts_server() {
        let raw = json!({ "links": { "grafana": "http://g", "port": 3000 }, "ssh": "prod", "services": ["db", "", null], "notes": 5 });
        let m = normalize(&raw);
        assert_eq!(m.links["port"], "3000");
        assert_eq!(m.ssh, vec!["prod"]);
        assert_eq!(m.services, vec!["db", "null"]);
        assert_eq!(m.notes, None);
        assert_eq!(
            serde_json::to_string(&m).unwrap(),
            r#"{"links":{"grafana":"http://g","port":"3000"},"ssh":["prod"],"services":["db","null"],"units":[],"docs":[]}"#
        );
    }
}
