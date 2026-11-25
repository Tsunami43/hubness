//! Catalog of projects and groups — a port of the parts of `apps/server/src/catalog.ts`
//! that need neither git nor a running agent.
//!
//! - Project = exactly one git repository. Appears only explicitly (import, new, clone).
//! - The projects folder is fixed (`HUBNESS_HOME/projects`); the hub never scans it on its
//!   own, repositories found there are only offered for import.
//! - Group = a set of projects (many-to-many) with its own workspace and sessions.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use hubness_core::{
    DiscoverResult, DiscoveredRepo, GroupDetail, HubSettings, PermissionMode, Project, ProjectGroup, ProjectKind, WorkspaceScopeKind,
};
use rusqlite::Connection;

use crate::config::Config;
use crate::db::{self, groups, kv, projects};
use crate::error::{ApiResult, HttpError};
use crate::paths;
use crate::workspaces;

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
pub fn assert_removable(db: &Connection, id: &str) -> ApiResult<Project> {
    let p = projects::get(db, id)?.ok_or_else(|| HttpError::not_found("Project not found"))?;
    if p.kind == ProjectKind::System {
        return Err(HttpError::bad_request("The system project can’t be removed"));
    }
    // TODO(rust-port): refuse with 409 while an agent works in the project (needs the runtime)
    Ok(p)
}

/// Remove a project from the hub. Repository files are untouched; project sessions are deleted.
pub fn remove_project(db: &Connection, config: &Config, id: &str) -> ApiResult<()> {
    assert_removable(db, id)?;
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
pub fn delete_group(db: &Connection, config: &Config, id: &str) -> ApiResult<()> {
    group_or_404(db, id)?;
    // TODO(rust-port): refuse with 409 while an agent works in the group (needs the runtime)
    workspaces::remove_workspaces(db, config, WorkspaceScopeKind::Group, id)?;
    groups::delete(db, id)?;
    Ok(())
}
