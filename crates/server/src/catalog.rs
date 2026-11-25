//! Catalog of projects and groups — a port of `apps/server/src/catalog.ts` (except clone,
//! which needs terminal jobs, and deleting project folders).
//!
//! - Project = exactly one git repository. Appears only explicitly (import, new, clone).
//! - The projects folder is fixed (`HUBNESS_HOME/projects`); the hub never scans it on its
//!   own, repositories found there are only offered for import.
//! - Group = a set of projects (many-to-many) with its own workspace and sessions.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use hubness_core::{
    DiscoverResult, DiscoveredRepo, GroupDetail, GroupRef, HubSettings, Language, PermissionMode, Project, ProjectDetail, ProjectGroup,
    ProjectKind, RepoInfo, RepoLinks, WorkspaceScopeKind,
};
use rusqlite::Connection;

use crate::AppState;
use crate::config::Config;
use crate::db::{self, groups, kv, projects, sessions};
use crate::error::{ApiResult, HttpError};
use crate::workspaces;
use crate::{context, git, git_providers, languages, paths};

const SKIP: &[&str] = &["node_modules", "target", "dist", "build", "vendor", "__pycache__", ".venv", "venv"];
const MAX_DEPTH: usize = 3;
const INSTRUCTIONS_MAX: usize = 20_000;

// ---------- settings ----------

pub fn settings(db: &Connection, config: &Config) -> ApiResult<HubSettings> {
    let perm = kv::get::<String>(db, db::PERMISSION_KEY)?.and_then(|s| PermissionMode::parse(&s));
    Ok(HubSettings {
        projects_root: paths::to_string(&config.projects_dir),
        default_permission: perm.unwrap_or(PermissionMode::Ask),
        default_instructions: kv::get::<String>(db, db::DEFAULT_INSTRUCTIONS_KEY)?.unwrap_or_default(),
        session_instructions: kv::get::<String>(db, db::SESSION_INSTRUCTIONS_KEY)?.unwrap_or_default(),
    })
}

pub fn set_default_permission(db: &Connection, mode: PermissionMode) -> ApiResult<()> {
    Ok(kv::set(db, db::PERMISSION_KEY, &mode.as_str())?)
}

/// Instructions new projects start with (existing projects keep theirs).
pub fn set_default_instructions(db: &Connection, text: &str) -> ApiResult<()> {
    if text.encode_utf16().count() > INSTRUCTIONS_MAX {
        return Err(HttpError::bad_request("defaultInstructions: at most 20000 characters"));
    }
    Ok(kv::set(db, db::DEFAULT_INSTRUCTIONS_KEY, &text.trim())?)
}

/// Instructions added to every session, whatever its scope.
pub fn set_session_instructions(db: &Connection, text: &str) -> ApiResult<()> {
    if text.encode_utf16().count() > INSTRUCTIONS_MAX {
        return Err(HttpError::bad_request("sessionInstructions: at most 20000 characters"));
    }
    Ok(kv::set(db, db::SESSION_INSTRUCTIONS_KEY, &text.trim())?)
}

// ---------- repository discovery ----------

fn is_repo_dir(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// git repositories in a folder (up to 3 levels; we don't descend into a repository).
pub fn scan_repos(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if is_repo_dir(dir) {
            out.push(dir.to_path_buf());
            return;
        }
        if depth >= MAX_DEPTH {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else { return };
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.') && !SKIP.contains(&n.as_str()))
            .collect();
        names.sort();
        for name in names {
            walk(&dir.join(name), depth + 1, out);
        }
    }
    let mut out = vec![];
    walk(root, 0, &mut out);
    out.sort();
    out
}

/// All hub projects (no disk scan). If the folder is gone, the project is marked missing.
pub fn list_projects(db: &Connection) -> ApiResult<Vec<Project>> {
    let mut list = projects::list(db)?;
    for p in &mut list {
        if !Path::new(&p.path).exists() {
            p.missing = Some(true);
        }
    }
    Ok(list)
}

/// Repositories in a folder (default: the projects folder) not yet added to the hub.
pub fn discover_repos(db: &Connection, config: &Config, path: Option<&str>) -> ApiResult<DiscoverResult> {
    let root = match path.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => paths::resolve(p),
        None => config.projects_dir.clone(),
    };
    if !root.is_dir() {
        return Err(HttpError::bad_request("Folder not found"));
    }
    let known: HashSet<String> = projects::list(db)?.into_iter().map(|p| p.path).collect();
    let repos = scan_repos(&root)
        .into_iter()
        .map(|r| paths::to_string(&r))
        .filter(|r| !known.contains(r))
        .map(|r| DiscoveredRepo { name: paths::basename(Path::new(&r)), path: r })
        .collect();
    Ok(DiscoverResult { root: paths::to_string(&root), repos })
}

/// Import existing repositories (explicit user choice). Already added ones are not duplicated.
pub fn import_projects(db: &Connection, raw_paths: &[String], group_id: Option<&str>) -> ApiResult<Vec<Project>> {
    if raw_paths.is_empty() {
        return Err(HttpError::bad_request("paths must be a non-empty list of folders"));
    }
    let group = match group_id.filter(|g| !g.is_empty()) {
        Some(id) => Some(group_or_404(db, id)?),
        None => None,
    };
    let mut dirs = vec![];
    for raw in raw_paths {
        let dir = paths::resolve(raw.trim());
        let shown = paths::to_string(&dir);
        if !dir.is_dir() {
            return Err(HttpError::bad_request(format!("Folder not found: {shown}")));
        }
        if !is_repo_dir(&dir) {
            return Err(HttpError::bad_request(format!("Not a git repository: {shown}")));
        }
        dirs.push(dir);
    }
    let mut out = vec![];
    for dir in dirs {
        let path = paths::to_string(&dir);
        let p = match projects::by_path(db, &path)? {
            Some(p) => p,
            None => projects::create(db, &paths::basename(&dir), &path, ProjectKind::Normal)?,
        };
        out.push(p);
    }
    if let Some(g) = group {
        let ids: Vec<String> = out.iter().map(|p| p.id.clone()).collect();
        add_to_group(db, &g, &ids)?;
    }
    Ok(out)
}

// ---------- project ----------

pub fn project_or_404(db: &Connection, id: &str) -> ApiResult<Project> {
    let p = projects::get(db, id)?.ok_or_else(|| HttpError::not_found("Project not found"))?;
    if !Path::new(&p.path).exists() {
        return Err(HttpError::new(410, format!("Project folder not found: {}", p.path)));
    }
    Ok(p)
}

#[derive(Clone, Copy)]
enum Field {
    Name,
    Description,
    Instructions,
}

impl Field {
    fn label(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Description => "description",
            Self::Instructions => "instructions",
        }
    }
    fn max(self) -> usize {
        match self {
            Self::Name => 120,
            Self::Description => 4000,
            Self::Instructions => 20_000,
        }
    }
}

/// Length check (in UTF-16 units, as JavaScript's `length`); names are trimmed.
fn text(v: Option<String>, field: Field) -> ApiResult<Option<String>> {
    let Some(v) = v else { return Ok(None) };
    if v.encode_utf16().count() > field.max() {
        return Err(HttpError::bad_request(format!("{} is too long (max {})", field.label(), field.max())));
    }
    Ok(Some(match field {
        Field::Name => v.trim().to_string(),
        _ => v,
    }))
}

pub struct ProjectPatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub instructions: Option<String>,
}

pub fn update_project(db: &Connection, id: &str, b: ProjectPatch) -> ApiResult<Project> {
    project_or_404(db, id)?;
    let name = text(b.name, Field::Name)?;
    if name.as_deref() == Some("") {
        return Err(HttpError::bad_request("Name is required"));
    }
    let patch = projects::Patch {
        name,
        description: text(b.description, Field::Description)?,
        instructions: text(b.instructions, Field::Instructions)?,
    };
    projects::update(db, id, patch)?.ok_or_else(|| HttpError::not_found("Project not found"))
}

/// Checks before removing a project from the hub.
pub fn assert_removable(db: &Connection, id: &str, is_active: &dyn Fn(&str) -> bool) -> ApiResult<Project> {
    let p = projects::get(db, id)?.ok_or_else(|| HttpError::not_found("Project not found"))?;
    if p.kind == ProjectKind::System {
        return Err(HttpError::bad_request("The system project can’t be removed"));
    }
    let all = sessions::list(db, sessions::Filter { project_id: Some(id), archived: sessions::Archived::All, ..Default::default() })?;
    if all.iter().any(|s| is_active(&s.id)) {
        return Err(HttpError::new(409, "An agent is working in this project — stop it first"));
    }
    Ok(p)
}

/// Remove a project from the hub. Repository files are untouched; project sessions are deleted.
pub fn remove_project(db: &Connection, config: &Config, id: &str, is_active: &dyn Fn(&str) -> bool) -> ApiResult<()> {
    assert_removable(db, id, is_active)?;
    workspaces::remove_workspaces(db, config, WorkspaceScopeKind::Project, id)?;
    projects::delete(db, id)?;
    Ok(())
}

// ---------- groups ----------

pub fn group_or_404(db: &Connection, id: &str) -> ApiResult<ProjectGroup> {
    groups::get(db, id)?.ok_or_else(|| HttpError::not_found("Group not found"))
}

fn members_of(db: &Connection, ids: &[String]) -> ApiResult<Vec<Project>> {
    let mut seen = HashSet::new();
    let mut out = vec![];
    for id in ids.iter().filter(|id| seen.insert(id.as_str())) {
        match projects::get(db, id)? {
            Some(p) if p.kind == ProjectKind::Normal => out.push(p),
            _ => return Err(HttpError::bad_request(format!("Unknown project: {id}"))),
        }
    }
    Ok(out)
}

fn add_to_group(db: &Connection, g: &ProjectGroup, ids: &[String]) -> ApiResult<()> {
    let mut all = g.project_ids.clone();
    for id in ids {
        if !all.contains(id) {
            all.push(id.clone());
        }
    }
    groups::update(db, &g.id, groups::Patch { project_ids: Some(all), ..Default::default() })?;
    Ok(())
}

/// Group storage folder: an absolute path, or empty/null (= the projects folder).
/// `None` — not given (keep the current value).
fn folder_of(v: Option<Option<String>>) -> ApiResult<Option<Option<String>>> {
    let Some(v) = v else { return Ok(None) };
    let raw = v.as_deref().map(str::trim).unwrap_or("");
    if raw.is_empty() {
        return Ok(Some(None));
    }
    if !Path::new(raw).is_absolute() {
        return Err(HttpError::bad_request("folder must be an absolute path"));
    }
    let abs = paths::resolve(raw);
    if abs.exists() && !abs.is_dir() {
        return Err(HttpError::bad_request("folder is not a directory"));
    }
    Ok(Some(Some(paths::to_string(&abs))))
}

pub fn group_projects(db: &Connection, g: &ProjectGroup) -> ApiResult<Vec<Project>> {
    let mut out = vec![];
    for id in &g.project_ids {
        if let Some(p) = projects::get(db, id)? {
            out.push(p);
        }
    }
    Ok(out)
}

pub fn group_detail(db: &Connection, config: &Config, id: &str) -> ApiResult<GroupDetail> {
    let g = group_or_404(db, id)?;
    let list = group_projects(db, &g)?;
    let workspace = workspaces::group_workspace(db, config, &g.id, &list)?;
    Ok(GroupDetail { group: g, projects: list, workspace })
}

#[derive(Default)]
pub struct GroupFields {
    pub name: Option<String>,
    pub description: Option<String>,
    pub instructions: Option<String>,
    pub folder: Option<Option<String>>,
    pub project_ids: Option<Vec<String>>,
}

pub fn create_group(db: &Connection, config: &Config, b: GroupFields) -> ApiResult<GroupDetail> {
    let name = text(Some(b.name.unwrap_or_default()), Field::Name)?.unwrap_or_default();
    if name.is_empty() {
        return Err(HttpError::bad_request("Name is required"));
    }
    let members = members_of(db, &b.project_ids.unwrap_or_default())?;
    let g = groups::create(
        db,
        groups::New {
            name,
            description: text(b.description, Field::Description)?.unwrap_or_default(),
            instructions: text(b.instructions, Field::Instructions)?.unwrap_or_default(),
            folder: folder_of(b.folder)?.flatten(),
            project_ids: members.into_iter().map(|m| m.id).collect(),
        },
    )?;
    group_detail(db, config, &g.id)
}

pub fn update_group(db: &Connection, config: &Config, id: &str, b: GroupFields) -> ApiResult<GroupDetail> {
    group_or_404(db, id)?;
    let name = text(b.name, Field::Name)?;
    if name.as_deref() == Some("") {
        return Err(HttpError::bad_request("Name is required"));
    }
    let project_ids = match b.project_ids {
        Some(ids) => Some(members_of(db, &ids)?.into_iter().map(|m| m.id).collect()),
        None => None,
    };
    groups::update(
        db,
        id,
        groups::Patch {
            name,
            description: text(b.description, Field::Description)?,
            instructions: text(b.instructions, Field::Instructions)?,
            folder: folder_of(b.folder)?,
            project_ids,
        },
    )?;
    group_detail(db, config, id)
}

/// Deletes a group, its workspace and group sessions. Projects and their sessions are untouched.
pub fn delete_group(db: &Connection, config: &Config, id: &str, is_active: &dyn Fn(&str) -> bool) -> ApiResult<()> {
    group_or_404(db, id)?;
    let all = sessions::list(db, sessions::Filter { group_id: Some(id), archived: sessions::Archived::All, ..Default::default() })?;
    if all.iter().any(|s| is_active(&s.id)) {
        return Err(HttpError::new(409, "An agent is working in this group — stop it first"));
    }
    workspaces::remove_workspaces(db, config, WorkspaceScopeKind::Group, id)?;
    groups::delete(db, id)?;
    Ok(())
}

// ---------- repository info (needs git) ----------

/// Repository info for lists (provider, remote, default branch, language) — refreshed once
/// per server start, in the background, one project at a time.
#[derive(Clone, Default)]
pub struct MetaRefresh {
    refreshed: Arc<Mutex<HashSet<String>>>,
    queue: Arc<tokio::sync::Mutex<()>>,
}

pub fn refresh_repo_meta_in_background(s: &AppState, list: &[Project]) {
    let todo: Vec<Project> = {
        let Ok(mut done) = s.meta.refreshed.lock() else { return };
        list.iter().filter(|p| p.kind == ProjectKind::Normal && p.missing.is_none() && done.insert(p.id.clone())).cloned().collect()
    };
    if todo.is_empty() {
        return;
    }
    let s = s.clone();
    tokio::spawn(async move {
        // the queue lock is fair: batches run one after another, in the order they were queued
        let _turn = s.meta.queue.lock().await;
        for p in todo {
            let _ = repo_info(&s, &p).await;
        }
    });
}

/// Repository info from git; cached in the project (provider, URL, default branch, language).
pub async fn repo_info(s: &AppState, p: &Project) -> ApiResult<RepoInfo> {
    let dir = Path::new(&p.path);
    let b = git::repo_basics(dir).await;
    let remote_url = b.remote_url.as_deref().map(git_providers::strip_credentials);
    let prov = git_providers::resolve_provider(remote_url.as_deref());
    let provider = prov.as_ref().map(|x| x.id);
    if p.git_provider != provider || p.repo_url != remote_url || p.default_branch != b.default_branch {
        projects::set_repo_meta(&*s.db()?, &p.id, provider, remote_url.as_deref(), b.default_branch.as_deref())?;
    }
    let links = prov.as_ref().and_then(|x| x.links());
    let languages = if b.is_repo { languages::repo_languages(dir).await } else { vec![] };
    let main = languages.first().map(|l| Language { name: l.name.clone(), color: l.color.clone() });
    if main != p.language {
        projects::set_language(&*s.db()?, &p.id, main.as_ref())?;
    }
    Ok(RepoInfo {
        languages,
        is_repo: b.is_repo,
        provider,
        provider_label: prov.as_ref().map(|x| x.label.to_string()),
        remote_url,
        links: links.map(|l| RepoLinks {
            branch: b.branch.as_deref().map(|br| l.branch(br)),
            new_pull_request: match (&b.branch, &b.default_branch) {
                (Some(br), Some(def)) if br != def => Some(l.new_pull_request(br, Some(def))),
                _ => None,
            },
            repo: l.repo,
        }),
        branch: b.branch,
        default_branch: b.default_branch,
    })
}

/// `GET /projects/:id`: the project with repository info, its groups and workspace.
pub async fn project_detail(s: &AppState, id: &str) -> ApiResult<ProjectDetail> {
    let p = project_or_404(&*s.db()?, id)?;
    let repo = repo_info(s, &p).await?;
    let db = s.db()?;
    let fresh = projects::get(&db, id)?.ok_or_else(|| HttpError::not_found("Project not found"))?;
    let mut refs = vec![];
    for gid in &fresh.group_ids {
        if let Some(g) = groups::get(&db, gid)? {
            refs.push(GroupRef { id: g.id, name: g.name });
        }
    }
    let workspace = workspaces::project_workspace(&db, &fresh)?;
    Ok(ProjectDetail { project: fresh, repo, groups: refs, workspace })
}

/// Refresh the cached repository info of a group (for the context preview).
pub async fn refresh_group_repos(s: &AppState, g: &ProjectGroup) -> ApiResult<()> {
    let list: Vec<Project> = group_projects(&*s.db()?, g)?.into_iter().filter(|p| Path::new(&p.path).exists()).collect();
    for r in futures_util::future::join_all(list.iter().map(|p| repo_info(s, p))).await {
        r?;
    }
    Ok(())
}

/// Context the agent receives in a group session (repository info from the project cache).
pub fn group_agent_context(db: &Connection, g: &ProjectGroup) -> ApiResult<String> {
    Ok(context::group_context(g, &group_projects(db, g)?))
}

// ---------- new projects ----------

/// Where to put a new project: the group folder (if set) or the projects folder.
fn storage_dir(config: &Config, group: Option<&ProjectGroup>) -> PathBuf {
    group.and_then(|g| g.folder.as_ref()).map(PathBuf::from).unwrap_or_else(|| config.projects_dir.clone())
}

/// Folder name from a project name: runs of anything but `[A-Za-z0-9_.-]` → `-`, no leading
/// `-`/`.`, no trailing `-` (`replace(/[^\w.-]+/g, '-').replace(/^[-.]+|-+$/g, '')`).
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut in_run = false;
    for c in crate::git_providers::js_trim(name).chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('-');
            in_run = true;
        }
    }
    out.trim_start_matches(['-', '.']).trim_end_matches('-').to_string()
}

fn target_dir(config: &Config, name: &str, group: Option<&ProjectGroup>) -> ApiResult<(PathBuf, String)> {
    let root = storage_dir(config, group);
    let slug = slug(name);
    if slug.is_empty() {
        return Err(HttpError::bad_request("Name is required"));
    }
    let dir = root.join(&slug);
    if dir.exists() {
        return Err(HttpError::new(409, format!("A folder named “{slug}” already exists in {}", paths::to_string(&root))));
    }
    Ok((dir, slug))
}

/// A new empty project in storage (group or projects folder): folder + `git init`.
pub async fn create_repo(s: &AppState, name: Option<&str>, group_id: Option<&str>) -> ApiResult<Project> {
    let (dir, slug, group) = {
        let db = s.db()?;
        let group = match group_id.filter(|g| !g.is_empty()) {
            Some(id) => Some(group_or_404(&db, id)?),
            None => None,
        };
        let (dir, slug) = target_dir(&s.config, name.unwrap_or(""), group.as_ref())?;
        (dir, slug, group)
    };
    fs::create_dir_all(&dir)?;
    let out = tokio::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .arg(&dir)
        .env_clear()
        .envs(crate::config::child_env())
        .stdin(std::process::Stdio::null())
        .output()
        .await?;
    if !out.status.success() {
        return Err(HttpError::new(
            500,
            format!("Command failed: git init -q {}\n{}", paths::to_string(&dir), String::from_utf8_lossy(&out.stderr)),
        ));
    }
    let db = s.db()?;
    let p = projects::create(&db, &slug, &paths::to_string(&dir), ProjectKind::Normal)?;
    if let Some(g) = group {
        // re-read: the members may have changed while git was running
        if let Some(g) = groups::get(&db, &g.id)? {
            add_to_group(&db, &g, std::slice::from_ref(&p.id))?;
        }
    }
    projects::get(&db, &p.id)?.ok_or_else(|| HttpError::not_found("Project not found"))
}

// ---------- deleting a project's folder ----------

fn real_or_resolved(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| paths::normalize(p))
}

/// `child` is `parent` or inside it.
fn inside(child: &Path, parent: &Path) -> bool {
    child.starts_with(parent)
}

/// The folder "delete with files" may remove: the project's own repository and nothing
/// wider. `None` — the folder is already gone (only the hub record is left to remove).
/// Refuses system folders (/, home, the hub's folders), folders holding the hub itself (its
/// data, its binary, the folder it runs from), folders that are not a git repository root,
/// and folders that contain another project.
pub fn deletable_folder(db: &Connection, config: &Config, p: &Project) -> ApiResult<Option<PathBuf>> {
    let path = Path::new(&p.path);
    if !path.exists() {
        return Ok(None);
    }
    let dir = real_or_resolved(path);
    let shown = dir.display();
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()));
    let guarded: Vec<PathBuf> =
        [home, config.hub_home.clone(), config.projects_dir.clone(), config.inbox_dir.clone(), config.home_project_dir.clone()]
            .iter()
            .map(|g| real_or_resolved(g))
            .collect();
    let depth = dir.components().filter(|c| matches!(c, std::path::Component::Normal(_))).count();
    if depth < 2 || guarded.contains(&dir) {
        return Err(HttpError::bad_request(format!("Refusing to delete {shown}: a system folder")));
    }
    let mut hub_paths = vec![real_or_resolved(&config.data_dir)];
    if let Ok(exe) = std::env::current_exe() {
        hub_paths.push(real_or_resolved(&exe));
    }
    if let Ok(cwd) = std::env::current_dir() {
        hub_paths.push(real_or_resolved(&cwd));
    }
    if hub_paths.iter().any(|h| inside(h, &dir)) {
        return Err(HttpError::bad_request(format!("Refusing to delete {shown}: it contains the hub itself")));
    }
    if !dir.is_dir() || !dir.join(".git").exists() {
        return Err(HttpError::bad_request(format!("Refusing to delete {shown}: not the root of a git repository")));
    }
    if let Some(nested) = projects::list(db)?.into_iter().find(|o| o.id != p.id && inside(&real_or_resolved(Path::new(&o.path)), &dir)) {
        return Err(HttpError::new(409, format!("Refusing to delete {shown}: project \"{}\" lives inside", nested.name)));
    }
    Ok(Some(dir))
}

#[cfg(test)]
mod tests {
    use super::slug;

    #[test]
    fn slugs_like_the_ts_server() {
        assert_eq!(slug("  My new repo!  "), "My-new-repo");
        assert_eq!(slug("..hidden"), "hidden");
        assert_eq!(slug("-a--b-"), "a--b");
        assert_eq!(slug("проект x"), "x");
        assert_eq!(slug("!!!"), "");
    }
}
