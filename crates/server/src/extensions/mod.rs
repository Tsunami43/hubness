//! What each agent is extended with (the Harness page, as `mcp-servers.ts`, `extensions.ts`
//! and the agents' `extensions.ts` modules in TS): MCP servers in the agents' own config
//! files — the matrix "server × agent", adding, connecting, disconnecting and deleting them
//! in the user configs — and the skills, rules files, subagents, commands and plugins the
//! agents read from their folders. Config edits keep everything else in the file, are atomic,
//! keep a one-time `<file>.hubness.bak` and never touch a file that doesn't parse.

mod catalog;
mod items;
mod mcp;
mod skills;

use std::path::{Path, PathBuf};

use axum::Router;
use axum::extract::{Path as UrlPath, Query, State};
use axum::routing::{delete, get, post, put};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use toml_edit::DocumentMut;

use crate::AppState;
use crate::db::projects;
use crate::error::{ApiResult, HttpError};
use crate::http::JsonBody;
use crate::paths::home;

use items::*;
use mcp::*;

const CLAUDE: &str = "claude-code";
const CODEX: &str = "codex";
const OPENCODE: &str = "opencode";
const GROK: &str = "grok";

/// The hub's own server: attached to every hub session by the hub itself.
const BUILTIN_MCP: &str = "hub";

// ---------- where the agents keep their config ----------

fn env_dir(k: &str) -> Option<PathBuf> {
    std::env::var(k).ok().filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn claude_dir() -> PathBuf {
    env_dir("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home().join(".claude"))
}

fn claude_json() -> PathBuf {
    match env_dir("CLAUDE_CONFIG_DIR") {
        Some(d) => d.join(".claude.json"),
        None => home().join(".claude.json"),
    }
}

fn codex_home() -> PathBuf {
    env_dir("CODEX_HOME").unwrap_or_else(|| home().join(".codex"))
}

fn opencode_dir() -> PathBuf {
    env_dir("XDG_CONFIG_HOME").unwrap_or_else(|| home().join(".config")).join("opencode")
}

fn grok_home() -> PathBuf {
    env_dir("GROK_HOME").unwrap_or_else(|| home().join(".grok"))
}

/// opencode.json, or opencode.jsonc when only that one exists.
fn opencode_file() -> PathBuf {
    let (json, jsonc) = (opencode_dir().join("opencode.json"), opencode_dir().join("opencode.jsonc"));
    if !json.exists() && jsonc.exists() { jsonc } else { json }
}

fn p(path: &Path) -> String {
    crate::paths::to_string(path)
}

// ---------- config files ----------

const MAX_CONFIG: u64 = 16 * 1024 * 1024;

fn unreadable(file: &Path, what: &str) -> HttpError {
    HttpError::new(409, format!("{} is not valid {what} — fix it or edit it by hand; the hub left it untouched", p(file)))
}

fn read_text(file: &Path) -> ApiResult<Option<String>> {
    if !file.exists() {
        return Ok(None);
    }
    if std::fs::metadata(file)?.len() > MAX_CONFIG {
        return Err(HttpError::new(409, format!("{} is too large to edit", p(file))));
    }
    Ok(Some(std::fs::read_to_string(file).map_err(|_| unreadable(file, "UTF-8 text"))?))
}

/// Atomic write through the real path (a symlink stays a symlink), keeping the file mode;
/// with `backup`, the first write keeps a copy of the original as `<file>.hubness.bak`.
fn write_config(file: &Path, text: &str, backup: bool) -> ApiResult<()> {
    use std::os::unix::fs::PermissionsExt;
    let exists = file.exists();
    let real = if exists { std::fs::canonicalize(file)? } else { file.to_path_buf() };
    if let Some(parent) = real.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut mode = 0o644;
    if exists {
        mode = std::fs::metadata(&real)?.permissions().mode() & 0o777;
        let bak = PathBuf::from(format!("{}.hubness.bak", p(file)));
        if backup && !bak.exists() {
            std::fs::copy(&real, &bak)?;
        }
    }
    let tmp = PathBuf::from(format!("{}.hubness-{}.tmp", p(&real), std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    std::fs::rename(&tmp, &real)?;
    Ok(())
}

fn read_json_file(file: &Path) -> ApiResult<Option<Map<String, Value>>> {
    let Some(text) = read_text(file)? else { return Ok(None) };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(m)) => Ok(Some(m)),
        _ => Err(unreadable(file, "JSON")),
    }
}

fn write_json_file(file: &Path, cfg: &Map<String, Value>) -> ApiResult<()> {
    write_config(file, &(serde_json::to_string_pretty(cfg).expect("json") + "\n"), true)
}

fn read_toml_file(file: &Path) -> ApiResult<Option<DocumentMut>> {
    let Some(text) = read_text(file)? else { return Ok(None) };
    text.parse::<DocumentMut>().map(Some).map_err(|_| unreadable(file, "TOML"))
}

// ---------- routes ----------

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/harness/mcp", get(mcp_get).post(mcp_post))
        .route("/harness/mcp/catalog", get(catalog_get))
        .route("/harness/extensions", get(extensions_all))
        .route("/harness/skills", get(skills_get).post(skills_post))
        .route("/harness/skills/import/preview", post(skills_preview))
        .route("/harness/skills/import", post(skills_import))
        .route("/harness/skills/{name}", get(skill_get).delete(skill_delete_route))
        .route("/harness/skills/{name}/agents/{kind}", put(skill_connect_route).delete(skill_disconnect_route))
        .route("/harness/mcp/{name}", delete(mcp_delete_route))
        .route("/harness/mcp/{name}/agents/{kind}", put(mcp_connect_route).delete(mcp_disconnect_route))
        .route("/harness/{kind}/extensions", get(extensions_route))
        .route("/harness/{kind}/rules", get(rules_get).put(rules_put))
        .route("/harness/{kind}/file", get(file_get))
}

#[derive(Deserialize)]
struct ExtQuery {
    project: Option<String>,
    path: Option<String>,
    q: Option<String>,
    cursor: Option<String>,
    agent: Option<String>,
}

/// The folder of `?project=<id>` (None without one).
fn project_dir(s: &AppState, q: &ExtQuery) -> ApiResult<Option<PathBuf>> {
    let Some(id) = q.project.as_deref().filter(|id| !id.is_empty()) else { return Ok(None) };
    let db = s.db()?;
    let p = projects::get(&db, id)?.ok_or_else(|| HttpError::not_found("Project not found"))?;
    Ok(Some(PathBuf::from(p.path)))
}

async fn mcp_get(State(s): State<AppState>, Query(q): Query<ExtQuery>) -> ApiResult<axum::Json<Value>> {
    let dir = project_dir(&s, &q)?;
    Ok(axum::Json(mcp_matrix(dir.as_deref())))
}

async fn mcp_post(State(s): State<AppState>, Query(q): Query<ExtQuery>, JsonBody(b): JsonBody<McpBody>) -> ApiResult<axum::Json<Value>> {
    let dir = project_dir(&s, &q)?;
    Ok(axum::Json(mcp_add(b, dir.as_deref())?))
}

async fn mcp_connect_route(
    State(s): State<AppState>,
    UrlPath((name, kind)): UrlPath<(String, String)>,
    Query(q): Query<ExtQuery>,
) -> ApiResult<axum::Json<Value>> {
    let dir = project_dir(&s, &q)?;
    Ok(axum::Json(mcp_connect(&name, &kind, dir.as_deref())?))
}

async fn mcp_disconnect_route(
    State(s): State<AppState>,
    UrlPath((name, kind)): UrlPath<(String, String)>,
    Query(q): Query<ExtQuery>,
) -> ApiResult<axum::Json<Value>> {
    let dir = project_dir(&s, &q)?;
    Ok(axum::Json(mcp_disconnect(&name, &kind, dir.as_deref())?))
}

async fn mcp_delete_route(
    State(s): State<AppState>,
    UrlPath(name): UrlPath<String>,
    Query(q): Query<ExtQuery>,
) -> ApiResult<axum::Json<Value>> {
    let dir = project_dir(&s, &q)?;
    Ok(axum::Json(mcp_delete(&name, dir.as_deref())?))
}

async fn catalog_get(Query(q): Query<ExtQuery>) -> ApiResult<axum::Json<Value>> {
    Ok(axum::Json(catalog::mcp_catalog(q.q.as_deref(), q.cursor.as_deref()).await?))
}

/// Every agent's extensions at once (the cross-agent Rules / Subagents / Commands / Plugins pages).
async fn extensions_all(State(s): State<AppState>, Query(q): Query<ExtQuery>) -> ApiResult<axum::Json<Value>> {
    let dir = project_dir(&s, &q)?;
    let agents: Vec<Value> = crate::harness::LABELS.iter().map(|(kind, _, _)| extensions_of(kind, dir.as_deref())).collect();
    Ok(axum::Json(json!({ "agents": agents })))
}

// ---------- skills across the agents ----------

async fn skills_get(State(s): State<AppState>, Query(q): Query<ExtQuery>) -> ApiResult<axum::Json<Value>> {
    let dir = project_dir(&s, &q)?;
    Ok(axum::Json(skills::skills_matrix(dir.as_deref())))
}

async fn skills_post(
    State(s): State<AppState>,
    Query(q): Query<ExtQuery>,
    JsonBody(b): JsonBody<skills::SkillBody>,
) -> ApiResult<axum::Json<Value>> {
    let dir = project_dir(&s, &q)?;
    Ok(axum::Json(skills::skill_create(&s.config().data_dir, b, dir.as_deref())?))
}

#[derive(Deserialize)]
struct PreviewBody {
    url: String,
}

async fn skills_preview(State(s): State<AppState>, JsonBody(b): JsonBody<PreviewBody>) -> ApiResult<axum::Json<Value>> {
    Ok(axum::Json(skills::skill_import_preview(&s.config().data_dir, &b.url).await?))
}

async fn skills_import(
    State(s): State<AppState>,
    Query(q): Query<ExtQuery>,
    JsonBody(b): JsonBody<skills::ImportBody>,
) -> ApiResult<axum::Json<Value>> {
    let dir = project_dir(&s, &q)?;
    Ok(axum::Json(skills::skill_import(&s.config().data_dir, b, dir.as_deref())?))
}

async fn skill_get(State(s): State<AppState>, UrlPath(name): UrlPath<String>, Query(q): Query<ExtQuery>) -> ApiResult<axum::Json<Value>> {
    let dir = project_dir(&s, &q)?;
    Ok(axum::Json(skills::skill_content(&name, q.agent.as_deref(), dir.as_deref())?))
}

async fn skill_delete_route(
    State(s): State<AppState>,
    UrlPath(name): UrlPath<String>,
    Query(q): Query<ExtQuery>,
) -> ApiResult<axum::Json<Value>> {
    let dir = project_dir(&s, &q)?;
    Ok(axum::Json(skills::skill_delete(&s.config().data_dir, &name, dir.as_deref())?))
}

async fn skill_connect_route(
    State(s): State<AppState>,
    UrlPath((name, kind)): UrlPath<(String, String)>,
    Query(q): Query<ExtQuery>,
) -> ApiResult<axum::Json<Value>> {
    let dir = project_dir(&s, &q)?;
    Ok(axum::Json(skills::skill_connect(&name, &kind, dir.as_deref())?))
}

async fn skill_disconnect_route(
    State(s): State<AppState>,
    UrlPath((name, kind)): UrlPath<(String, String)>,
    Query(q): Query<ExtQuery>,
) -> ApiResult<axum::Json<Value>> {
    let dir = project_dir(&s, &q)?;
    Ok(axum::Json(skills::skill_disconnect(&s.config().data_dir, &name, &kind, dir.as_deref())?))
}

async fn extensions_route(
    State(s): State<AppState>,
    UrlPath(kind): UrlPath<String>,
    Query(q): Query<ExtQuery>,
) -> ApiResult<axum::Json<Value>> {
    let kind = agent_or_404(&kind)?;
    let dir = project_dir(&s, &q)?;
    Ok(axum::Json(extensions_of(kind, dir.as_deref())))
}

async fn rules_get(State(s): State<AppState>, UrlPath(kind): UrlPath<String>, Query(q): Query<ExtQuery>) -> ApiResult<axum::Json<Value>> {
    let kind = agent_or_404(&kind)?;
    let dir = project_dir(&s, &q)?;
    let path = listed_rules(kind, dir.as_deref(), q.path.as_deref())?;
    if !path.exists() {
        return Ok(axum::Json(json!({ "path": p(&path), "exists": false, "content": "" })));
    }
    if std::fs::metadata(&path)?.len() > MAX_RULES {
        return Err(HttpError::new(413, "The file is too large to edit here"));
    }
    let content = std::fs::read_to_string(&path).map_err(|_| HttpError::new(422, "The file is not UTF-8 text"))?;
    Ok(axum::Json(json!({ "path": p(&path), "exists": true, "content": content })))
}

/// A skill, subagent or command file the listing names, read-only (markdown files only: an
/// agent defined inside a JSON config is never served, the config may hold secrets).
async fn file_get(State(s): State<AppState>, UrlPath(kind): UrlPath<String>, Query(q): Query<ExtQuery>) -> ApiResult<axum::Json<Value>> {
    let kind = agent_or_404(&kind)?;
    let dir = project_dir(&s, &q)?;
    let path = q.path.as_deref().filter(|p| !p.is_empty()).ok_or_else(|| HttpError::bad_request("path is required"))?;
    let ext = extensions_of(kind, dir.as_deref());
    let listed = ["skills", "agents", "commands"].iter().any(|k| ext[*k].as_array().is_some_and(|a| a.iter().any(|x| x["path"] == path)));
    if !listed || !path.ends_with(".md") {
        return Err(HttpError::new(403, "Not an extension file of this agent"));
    }
    let file = PathBuf::from(path);
    if !is_file(&file) {
        return Ok(axum::Json(json!({ "path": path, "exists": false, "content": "" })));
    }
    if std::fs::metadata(&file)?.len() > MAX_RULES {
        return Err(HttpError::new(413, "The file is too large to show here"));
    }
    let content = std::fs::read_to_string(&file).map_err(|_| HttpError::new(422, "The file is not UTF-8 text"))?;
    Ok(axum::Json(json!({ "path": path, "exists": true, "content": content })))
}

#[derive(Deserialize)]
struct RulesBody {
    content: String,
}

async fn rules_put(
    State(s): State<AppState>,
    UrlPath(kind): UrlPath<String>,
    Query(q): Query<ExtQuery>,
    JsonBody(b): JsonBody<RulesBody>,
) -> ApiResult<axum::Json<Value>> {
    let kind = agent_or_404(&kind)?;
    let dir = project_dir(&s, &q)?;
    let path = listed_rules(kind, dir.as_deref(), q.path.as_deref())?;
    if b.content.len() as u64 > MAX_RULES {
        return Err(HttpError::new(413, "content: at most 1 MB"));
    }
    write_config(&path, &b.content, false)?;
    Ok(axum::Json(json!({ "path": p(&path), "exists": true, "content": b.content })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_subset() {
        let (f, body) = parse_frontmatter(
            "---\nname: pdf\ndescription: >\n  Read PDFs.\n  Use it.\ntools:\n  - Read\nq: 'it''s'\nc: plain # comment\n---\n# Body\n",
        );
        assert_eq!(field(&f, "name"), "pdf");
        assert_eq!(field(&f, "description"), "Read PDFs. Use it.");
        assert_eq!(field(&f, "tools"), "");
        assert_eq!(field(&f, "q"), "it's");
        assert_eq!(field(&f, "c"), "plain");
        assert_eq!(body, "# Body\n");
        assert!(parse_frontmatter("no frontmatter").0.is_empty());
        assert!(parse_frontmatter("---\nunterminated: yes\n").0.is_empty());
        let (lit, _) = parse_frontmatter("---\nd: |\n  one\n  two\n---\n");
        assert_eq!(field(&lit, "d"), "one\ntwo");
    }

    #[test]
    fn toml_edits_keep_the_rest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("config.toml");
        std::fs::write(&file, "# top\nmodel = \"x\"\n\n[mcp_servers.a]\ncommand = \"a\"\n\n[mcp_servers.a.env]\nK = \"v\"\n\n# keep me\n[profiles.p]\nmodel = \"y\"\n").expect("write");
        let def = McpDef {
            name: "b".into(),
            transport: "http",
            command: None,
            args: vec![],
            env: vec![],
            url: Some("https://b.example".into()),
            headers: vec![("Authorization".into(), "t".into())],
            enabled: true,
        };
        put_def(CODEX, &file, &def).expect("put");
        assert!(remove_def(CODEX, &file, "a").expect("remove"));
        let text = std::fs::read_to_string(&file).expect("read");
        assert!(text.contains("# top") && text.contains("# keep me") && text.contains("[profiles.p]"), "{text}");
        assert!(!text.contains("mcp_servers.a"), "{text}");
        let defs = read_user(CODEX, &file).expect("read");
        assert_eq!(defs, vec![def]);
        assert!(std::fs::read_to_string(dir.path().join("config.toml.hubness.bak")).expect("bak").contains("[mcp_servers.a]"));
        std::fs::write(&file, "broken = \n").expect("write");
        assert_eq!(put_def(CODEX, &file, &defs[0]).map_err(|e| e.status.as_u16()), Err(409));
    }
}
