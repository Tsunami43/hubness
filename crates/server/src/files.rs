//! Project files — a port of `fsops.ts` and `routes/files.ts`: tree, read / write,
//! create / rename / delete, the file list, server folder browsing.
//!
//! Every path is checked to stay inside the project root (including via symlinks).

use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use axum::Router;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use hubness_core::{BrowseResponse, FileContent, FsEntry};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::error::{ApiResult, HttpError};
use crate::http::{JsonBody, blocking};
use crate::{AppState, catalog, paths};

const HIDDEN_IN_TREE: &[&str] = &[".git"];
/// Outside a git repository, the usual build and dependency folders are treated as internal.
const IGNORED_FALLBACK: &[&str] = &["node_modules", "target", "dist", ".next", ".venv", "venv", "__pycache__", ".cache"];
const MAX_FILE: u64 = 512 * 1024;

fn outside() -> HttpError {
    HttpError::bad_request("Path is outside the project")
}

fn real(p: &Path) -> ApiResult<PathBuf> {
    fs::canonicalize(p).map_err(|_| HttpError::not_found("Folder not found"))
}

fn rel_to(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).map(paths::to_string).unwrap_or_default()
}

/// A path inside the project root; escaping the root (including via symlinks) is forbidden.
pub fn safe_join(root: &Path, rel: &str) -> ApiResult<PathBuf> {
    let root_real = real(root)?;
    let target = paths::normalize(&root_real.join(if rel.is_empty() { "." } else { rel }));
    let resolved = if target.exists() { fs::canonicalize(&target)? } else { target };
    if resolved != root_real && !resolved.starts_with(&root_real) {
        return Err(outside());
    }
    Ok(resolved)
}

/// Directories first, then by name (case-insensitive like a locale compare, ties by bytes).
fn by_dir_then_name(a: &FsEntry, b: &FsEntry) -> Ordering {
    b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())).then_with(|| a.name.cmp(&b.name))
}

/// Which paths git ignores (`git check-ignore`); `None` — the folder is not in a repository.
fn git_ignored(cwd: &Path, names: &[String]) -> Option<HashSet<String>> {
    if names.is_empty() {
        return Some(HashSet::new());
    }
    let mut child = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["check-ignore", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let input = names.join("\n");
    let mut stdin = child.stdin.take()?;
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let out = child.wait_with_output().ok()?;
    let _ = writer.join();
    match out.status.code() {
        Some(0) => Some(String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.is_empty()).map(String::from).collect()),
        // 1 — nothing is ignored; 128 — not a repository
        Some(1) => Some(HashSet::new()),
        _ => None,
    }
}

pub fn list_tree(root: &Path, rel: &str) -> ApiResult<Vec<FsEntry>> {
    let abs = safe_join(root, rel)?;
    let root_real = real(root)?;
    let mut entries: Vec<FsEntry> = fs::read_dir(&abs)
        .map_err(|_| HttpError::not_found("Folder not found"))?
        .flatten()
        .filter(|d| !HIDDEN_IN_TREE.contains(&d.file_name().to_string_lossy().as_ref()))
        .map(|d| {
            let p = abs.join(d.file_name());
            let mut dir = d.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let mut size = None;
            if let Ok(m) = fs::metadata(&p) {
                dir = m.is_dir();
                size = (!dir).then_some(m.len());
            }
            FsEntry { name: d.file_name().to_string_lossy().into_owned(), path: rel_to(&root_real, &p), dir, size, ignored: None }
        })
        .collect();
    entries.sort_by(by_dir_then_name);
    let probe: Vec<String> = entries.iter().map(|e| if e.dir { format!("{}/", e.name) } else { e.name.clone() }).collect();
    let ignored = git_ignored(&abs, &probe);
    for (e, key) in entries.iter_mut().zip(probe) {
        e.ignored = Some(match &ignored {
            Some(set) => set.contains(&key) || set.contains(&e.name),
            None => IGNORED_FALLBACK.contains(&e.name.as_str()),
        });
    }
    Ok(entries)
}

fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

pub fn read_file_safe(root: &Path, rel: &str) -> ApiResult<FileContent> {
    let abs = safe_join(root, rel)?;
    let meta = fs::metadata(&abs).map_err(|_| HttpError::not_found("File not found"))?;
    if meta.is_dir() {
        return Err(HttpError::bad_request("This is a folder"));
    }
    let len = meta.len().min(MAX_FILE);
    let mut buf = Vec::with_capacity(len as usize);
    fs::File::open(&abs)?.take(len).read_to_end(&mut buf)?;
    let binary = buf[..buf.len().min(8000)].contains(&0);
    let truncated = meta.len() > MAX_FILE;
    Ok(FileContent {
        path: rel.to_string(),
        size: meta.len(),
        binary,
        truncated,
        content: if binary { String::new() } else { String::from_utf8_lossy(&buf).into_owned() },
        hash: (!binary && !truncated).then(|| sha256_hex(&buf)),
    })
}

/// Save a text file from the editor. Only existing regular files inside the root, never
/// inside .git, text only and no larger than what the viewer can load. With `base_hash` the
/// save is refused (409) if the file changed since it was opened — an agent may be editing it
/// too. The write is atomic (temp file + rename) and keeps the file mode.
pub fn write_file_safe(root: &Path, rel: &str, content: &str, base_hash: Option<&str>) -> ApiResult<FileContent> {
    let abs = safe_join(root, rel)?;
    let inside = rel_to(&real(root)?, &abs);
    if inside.is_empty() || inside.split('/').any(|p| p == ".git") {
        return Err(HttpError::bad_request("This path can’t be edited"));
    }
    if !abs.exists() {
        return Err(HttpError::not_found("File not found"));
    }
    let meta = fs::metadata(&abs)?;
    if !meta.is_file() {
        return Err(HttpError::bad_request("Not a regular file"));
    }
    if content.len() as u64 > MAX_FILE {
        return Err(HttpError::new(413, "The file is too large to edit here"));
    }
    if content.contains('\0') {
        return Err(HttpError::bad_request("Binary content is not allowed"));
    }
    let current = fs::read(&abs)?;
    if current[..current.len().min(8000)].contains(&0) {
        return Err(HttpError::bad_request("Binary files can’t be edited"));
    }
    if let Some(h) = base_hash.filter(|h| !h.is_empty())
        && sha256_hex(&current) != h
    {
        return Err(HttpError::new(409, "The file changed on disk since you opened it"));
    }
    let mode = meta.permissions().mode() & 0o7777;
    let name = paths::basename(&abs);
    let millis = jiff::Timestamp::now().as_millisecond();
    let tmp = abs.with_file_name(format!(".{name}.hubness-{}-{millis}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        fs::write(&tmp, content.as_bytes())?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
        fs::rename(&tmp, &abs)
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    read_file_safe(root, rel)
}

// ---------- file operations (Explorer: new / rename / move / delete) ----------

/// An entry path inside the project, checked lexically for the entry itself (a symlink is
/// renamed/deleted as a link, never its target) and by realpath for its parent folder.
/// Never the root itself, never inside .git.
fn entry_path(root: &Path, rel: &str) -> ApiResult<PathBuf> {
    let root_real = real(root)?;
    let clean = rel.replace('\\', "/");
    let clean = clean.trim_matches('/');
    if clean.is_empty() || clean.split('/').any(|p| p.is_empty() || p == "." || p == "..") {
        return Err(HttpError::bad_request("Invalid path"));
    }
    if clean.split('/').any(|p| p == ".git") {
        return Err(HttpError::bad_request("This path can’t be changed"));
    }
    let abs = root_real.join(clean);
    let mut parent = abs.parent().expect("inside the root").to_path_buf();
    // the parent may not exist yet (new nested path): check the nearest existing ancestor
    let mut probe = parent.clone();
    while !probe.exists() {
        probe = probe.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/"));
    }
    let probe_real = fs::canonicalize(&probe)?;
    if probe_real != root_real && !probe_real.starts_with(&root_real) {
        return Err(outside());
    }
    if parent.exists() {
        parent = fs::canonicalize(&parent)?;
    }
    Ok(parent.join(abs.file_name().expect("has a name")))
}

fn exists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

fn conflict() -> HttpError {
    HttpError::new(409, "A file or folder with this name already exists")
}

/// New empty file or folder (parent folders are created).
pub fn create_entry(root: &Path, rel: &str, dir: bool) -> ApiResult<String> {
    let abs = entry_path(root, rel)?;
    if exists(&abs) {
        return Err(conflict());
    }
    fs::create_dir_all(abs.parent().expect("has a parent"))?;
    if dir {
        fs::create_dir(&abs)?;
    } else {
        fs::OpenOptions::new().write(true).create_new(true).open(&abs)?;
    }
    Ok(rel_to(&real(root)?, &abs))
}

/// Rename or move a file/folder inside the project; never overwrites.
pub fn rename_entry(root: &Path, from: &str, to: &str) -> ApiResult<String> {
    let src = entry_path(root, from)?;
    let dst = entry_path(root, to)?;
    if !exists(&src) {
        return Err(HttpError::not_found("File not found"));
    }
    if src == dst {
        return Ok(rel_to(&real(root)?, &dst));
    }
    if exists(&dst) {
        return Err(conflict());
    }
    if dst.starts_with(&src) {
        return Err(HttpError::bad_request("Can’t move a folder into itself"));
    }
    fs::create_dir_all(dst.parent().expect("has a parent"))?;
    fs::rename(&src, &dst)?;
    Ok(rel_to(&real(root)?, &dst))
}

/// Delete a file or a folder (recursively). Git keeps tracked files recoverable.
pub fn delete_entry(root: &Path, rel: &str) -> ApiResult<()> {
    let abs = entry_path(root, rel)?;
    let Ok(meta) = fs::symlink_metadata(&abs) else { return Err(HttpError::not_found("File not found")) };
    if meta.is_dir() {
        fs::remove_dir_all(&abs)?
    } else {
        fs::remove_file(&abs)?
    }
    Ok(())
}

/// Browse server folders — for picking a project folder.
pub fn browse(path: Option<&str>) -> ApiResult<BrowseResponse> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    let abs = paths::resolve(path.filter(|p| !p.is_empty()).unwrap_or(&home));
    if !abs.is_dir() {
        return Err(HttpError::bad_request("Folder not found"));
    }
    let read = fs::read_dir(&abs).map_err(|_| HttpError::new(403, "No access to the folder"))?;
    let mut entries: Vec<FsEntry> = read
        .flatten()
        .filter(|d| d.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|d| d.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.') && n != "node_modules")
        .map(|n| FsEntry { path: paths::to_string(&abs.join(&n)), name: n, dir: true, size: None, ignored: None })
        .collect();
    entries.sort_by(by_dir_then_name);
    let parent = abs.parent().map(paths::to_string).filter(|p| Path::new(p) != abs);
    Ok(BrowseResponse { is_git_repo: abs.join(".git").exists(), path: paths::to_string(&abs), parent, entries })
}

/// All project files for search: git ls-files (tracked + untracked, excluding ignored);
/// outside git — a folder walk.
pub fn list_files(root: &Path, limit: usize) -> ApiResult<Vec<String>> {
    let root_real = real(root)?;
    let out = Command::new("git").arg("-C").arg(&root_real).args(["ls-files", "-co", "--exclude-standard"]).stderr(Stdio::null()).output();
    if let Ok(o) = out
        && o.status.success()
    {
        return Ok(String::from_utf8_lossy(&o.stdout).lines().filter(|l| !l.is_empty()).take(limit).map(String::from).collect());
    }
    fn walk(root: &Path, dir: &Path, limit: usize, out: &mut Vec<String>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for d in rd.flatten() {
            if out.len() >= limit {
                return;
            }
            let name = d.file_name().to_string_lossy().into_owned();
            if HIDDEN_IN_TREE.contains(&name.as_str()) || IGNORED_FALLBACK.contains(&name.as_str()) {
                continue;
            }
            let p = dir.join(&name);
            if d.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                walk(root, &p, limit, out);
            } else {
                out.push(rel_to(root, &p));
            }
        }
    }
    let mut res = vec![];
    walk(&root_real, &root_real, limit, &mut res);
    Ok(res)
}

// ---------- routes ----------

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/fs/browse", get(browse_route))
        .route("/projects/{id}/tree", get(tree_route))
        .route("/projects/{id}/files", get(files_route))
        .route("/projects/{id}/file", get(read_route).put(write_route))
        .route("/projects/{id}/fs/create", post(create_route))
        .route("/projects/{id}/fs/rename", post(rename_route))
        .route("/projects/{id}/fs/delete", post(delete_route))
        .route("/projects/{id}/search", get(search_route))
        .route("/watch", get(watch_route))
}

/// The project repository folder (a project = exactly one repository).
fn root_of(s: &AppState, id: &str) -> ApiResult<PathBuf> {
    Ok(PathBuf::from(catalog::project_or_404(&*s.db()?, id)?.path))
}

#[derive(Deserialize)]
struct PathQuery {
    path: Option<String>,
}

async fn browse_route(Query(q): Query<PathQuery>) -> ApiResult<axum::Json<BrowseResponse>> {
    blocking(move || browse(q.path.as_deref())).await
}

async fn tree_route(
    State(s): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Query(q): Query<PathQuery>,
) -> ApiResult<axum::Json<Vec<FsEntry>>> {
    let root = root_of(&s, &id)?;
    blocking(move || list_tree(&root, q.path.as_deref().unwrap_or(""))).await
}

async fn files_route(State(s): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<axum::Json<Value>> {
    let root = root_of(&s, &id)?;
    blocking(move || Ok(json!({ "files": list_files(&root, 20_000)? }))).await
}

async fn read_route(
    State(s): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Query(q): Query<PathQuery>,
) -> ApiResult<axum::Json<FileContent>> {
    let root = root_of(&s, &id)?;
    let rel = q.path.filter(|p| !p.is_empty()).ok_or_else(|| HttpError::bad_request("path is required"))?;
    blocking(move || read_file_safe(&root, &rel)).await
}

fn require(v: &str, field: &str) -> ApiResult<()> {
    if v.is_empty() {
        return Err(HttpError::bad_request(format!("{field}: {field} is required")));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WriteBody {
    path: String,
    content: String,
    base_hash: Option<String>,
}

async fn write_route(
    State(s): State<AppState>,
    UrlPath(id): UrlPath<String>,
    JsonBody(b): JsonBody<WriteBody>,
) -> ApiResult<axum::Json<FileContent>> {
    require(&b.path, "path")?;
    let root = root_of(&s, &id)?;
    blocking(move || write_file_safe(&root, &b.path, &b.content, b.base_hash.as_deref())).await
}

#[derive(Deserialize)]
struct CreateBody {
    path: String,
    dir: Option<bool>,
}

async fn create_route(
    State(s): State<AppState>,
    UrlPath(id): UrlPath<String>,
    JsonBody(b): JsonBody<CreateBody>,
) -> ApiResult<(StatusCode, axum::Json<Value>)> {
    require(&b.path, "path")?;
    let root = root_of(&s, &id)?;
    let path = blocking(move || create_entry(&root, &b.path, b.dir.unwrap_or(false))).await?.0;
    Ok((StatusCode::CREATED, axum::Json(json!({ "path": path }))))
}

#[derive(Deserialize)]
struct RenameBody {
    from: String,
    to: String,
}

async fn rename_route(
    State(s): State<AppState>,
    UrlPath(id): UrlPath<String>,
    JsonBody(b): JsonBody<RenameBody>,
) -> ApiResult<axum::Json<Value>> {
    require(&b.from, "from")?;
    require(&b.to, "to")?;
    let root = root_of(&s, &id)?;
    blocking(move || Ok(json!({ "path": rename_entry(&root, &b.from, &b.to)? }))).await
}

#[derive(Deserialize)]
struct DeleteBody {
    path: String,
}

async fn delete_route(
    State(s): State<AppState>,
    UrlPath(id): UrlPath<String>,
    JsonBody(b): JsonBody<DeleteBody>,
) -> ApiResult<axum::Json<Value>> {
    require(&b.path, "path")?;
    let root = root_of(&s, &id)?;
    blocking(move || {
        delete_entry(&root, &b.path)?;
        Ok(json!({ "ok": true }))
    })
    .await
}

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
    regex: Option<String>,
    case: Option<String>,
    word: Option<String>,
    include: Option<String>,
    exclude: Option<String>,
}

async fn search_route(
    State(s): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Query(q): Query<SearchQuery>,
) -> ApiResult<axum::Json<hubness_core::SearchResult>> {
    let root = root_of(&s, &id)?;
    let on = |v: &Option<String>| v.as_deref() == Some("1");
    let opts = crate::search::Options {
        regex: on(&q.regex),
        case_sensitive: on(&q.case),
        word: on(&q.word),
        q: q.q.unwrap_or_default(),
        include: q.include,
        exclude: q.exclude,
    };
    Ok(axum::Json(crate::search::search_files(&root, opts).await?))
}

#[derive(Deserialize)]
struct WatchQuery {
    projects: Option<String>,
}

/// Live file changes of the given projects (`?projects=id1,id2`): one stream per workbench,
/// so a group with many repositories does not use up the browser's connection limit.
async fn watch_route(
    State(s): State<AppState>,
    Query(q): Query<WatchQuery>,
) -> ApiResult<axum::response::sse::Sse<impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>>> {
    use axum::response::sse::{Event, Sse};
    use futures_util::StreamExt;
    let mut ids: Vec<String> = vec![];
    for id in q.projects.as_deref().unwrap_or("").split(',').filter(|i| !i.is_empty()) {
        if !ids.iter().any(|x| x == id) {
            ids.push(id.to_string());
        }
    }
    ids.truncate(50);
    let list = {
        let db = s.db()?;
        ids.iter().map(|id| catalog::project_or_404(&db, id)).collect::<ApiResult<Vec<_>>>()?
    };
    let streams = list.into_iter().map(|p| {
        let sub = s.watches.subscribe(Path::new(&p.path));
        futures_util::stream::unfold((sub, p.id), |(mut sub, pid)| async move {
            loop {
                match sub.rx.recv().await {
                    Ok(c) => {
                        let msg = hubness_core::FileChangeMsg {
                            project_id: pid.clone(),
                            paths: c.paths,
                            git: c.git,
                            overflow: c.overflow.then_some(true),
                        };
                        let ev = Event::default().data(serde_json::to_string(&msg).unwrap_or_default());
                        return Some((Ok(ev), (sub, pid)));
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
                }
            }
        })
        .boxed()
    });
    let changes = futures_util::stream::select_all(streams);
    let ping = tokio_stream::wrappers::IntervalStream::new(tokio::time::interval_at(
        tokio::time::Instant::now() + std::time::Duration::from_secs(25),
        std::time::Duration::from_secs(25),
    ))
    .map(|_| Ok(Event::default().event("ping").data("")));
    Ok(Sse::new(futures_util::stream::select(changes, ping)))
}
