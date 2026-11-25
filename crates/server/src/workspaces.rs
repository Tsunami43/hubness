//! Workspaces — a port of `apps/server/src/workspaces/` (currently only the local provider).
//!
//! Local provider: the code already lives on this machine.
//! - project: the workspace is the repository itself (in place, no copies);
//! - group: folder `<data>/groups/<id>/` with symlinks to the member repositories.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use hubness_core::{Project, Workspace, WorkspaceCapabilities, WorkspaceScopeKind, WorkspaceStatus, WorkspaceStrategy};
use rusqlite::Connection;

use crate::config::Config;
use crate::db::workspaces::{self, Upsert, WorkspaceRow};
use crate::error::ApiResult;
use crate::paths;

pub const LOCAL_PROVIDER: &str = "local";
const LOCAL_CAPABILITIES: WorkspaceCapabilities = WorkspaceCapabilities { lifecycle: false, clones: false };

fn with_capabilities(r: WorkspaceRow) -> Workspace {
    Workspace {
        id: r.id,
        scope: r.scope,
        scope_id: r.scope_id,
        provider: r.provider,
        strategy: r.strategy,
        path: r.path,
        status: r.status,
        status_detail: r.status_detail,
        capabilities: LOCAL_CAPABILITIES,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }
}

/// Unique link names: when folder names collide — name-2, name-3…
pub fn link_names(paths: &[&str]) -> Vec<String> {
    let mut used = HashSet::new();
    paths
        .iter()
        .map(|p| {
            let base = paths::basename(Path::new(p));
            let mut name = base.clone();
            let mut i = 2;
            while used.contains(&name) {
                name = format!("{base}-{i}");
                i += 1;
            }
            used.insert(name.clone());
            name
        })
        .collect()
}

pub fn project_workspace(db: &Connection, project: &Project) -> ApiResult<Workspace> {
    let ok = Path::new(&project.path).exists();
    let row = workspaces::upsert(
        db,
        Upsert {
            scope: WorkspaceScopeKind::Project,
            scope_id: &project.id,
            provider: LOCAL_PROVIDER,
            strategy: WorkspaceStrategy::Single,
            path: &project.path,
            status: if ok { WorkspaceStatus::Ready } else { WorkspaceStatus::Missing },
            status_detail: if ok { None } else { Some("Repository folder not found") },
        },
    )?;
    Ok(with_capabilities(row))
}

/// Sync the group's symlink folder with its members and save the state.
pub fn group_workspace(db: &Connection, config: &Config, group_id: &str, projects: &[Project]) -> ApiResult<Workspace> {
    let dir = config.groups_dir().join(group_id);
    fs::create_dir_all(&dir)?;
    let targets: Vec<&str> = projects.iter().map(|p| p.path.as_str()).collect();
    let want: HashMap<String, &str> = link_names(&targets).into_iter().zip(targets.iter().copied()).collect();
    // remove extra/stale links, create needed ones; leave other files alone
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let current = fs::read_link(entry.path())?;
        if want.get(&name).map(Path::new) != Some(current.as_path()) {
            fs::remove_file(entry.path())?;
        }
    }
    for (name, target) in &want {
        let link = dir.join(name);
        if fs::symlink_metadata(&link).is_err() {
            std::os::unix::fs::symlink(target, &link)?;
        }
    }
    let missing: Vec<&str> = projects.iter().filter(|p| !Path::new(&p.path).exists()).map(|p| p.name.as_str()).collect();
    let detail = (!missing.is_empty()).then(|| format!("Missing repositories: {}", missing.join(", ")));
    let row = workspaces::upsert(
        db,
        Upsert {
            scope: WorkspaceScopeKind::Group,
            scope_id: group_id,
            provider: LOCAL_PROVIDER,
            strategy: WorkspaceStrategy::Multi,
            path: &paths::to_string(&dir),
            status: if missing.is_empty() { WorkspaceStatus::Ready } else { WorkspaceStatus::Missing },
            status_detail: detail.as_deref(),
        },
    )?;
    Ok(with_capabilities(row))
}

/// Delete the scope's environments (never the user's repositories) and their records.
pub fn remove_workspaces(db: &Connection, config: &Config, scope: WorkspaceScopeKind, scope_id: &str) -> ApiResult<()> {
    if let Some(row) = workspaces::find(db, scope, scope_id, LOCAL_PROVIDER)? {
        // a project's workspace is the repository itself; only group folders are ours
        if scope == WorkspaceScopeKind::Group {
            let dir = paths::resolve(&row.path);
            let root = paths::normalize(&config.groups_dir());
            if dir.starts_with(&root) && dir != root && dir.exists() {
                fs::remove_dir_all(&dir)?;
            }
        }
    }
    workspaces::delete_scope(db, scope, scope_id)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colliding_folder_names_get_suffixes() {
        assert_eq!(link_names(&["/a/api", "/b/api", "/c/web", "/d/api"]), vec!["api", "api-2", "web", "api-3"]);
    }
}
