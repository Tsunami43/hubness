//! Data access: the hub database (SQLite, one file in the data folder). The table layout
//! and JSON stored in columns match the TypeScript server, so both can use one file.

pub mod migrations;
pub mod sessions;

use std::path::Path;

use hubness_core::{GitProviderId, Language, Project, ProjectGroup, ProjectKind, WorkspaceScopeKind, WorkspaceStatus, WorkspaceStrategy};
use rusqlite::{Connection, OptionalExtension, Row};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Open the database, enable WAL and foreign keys, apply pending migrations.
pub fn open(path: &Path) -> anyhow::Result<Connection> {
    let db = Connection::open(path)?;
    init(&db)?;
    Ok(db)
}

pub fn init(db: &Connection) -> anyhow::Result<()> {
    db.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    migrations::migrate(db, migrations::MIGRATIONS)?;
    Ok(())
}

/// ISO 8601 timestamp with milliseconds, as JavaScript's `Date.toISOString()`.
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Run `f` in a transaction: committed if it returns `Ok`, rolled back otherwise.
pub fn tx<T, E: From<rusqlite::Error>>(db: &Connection, f: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
    db.execute_batch("BEGIN")?;
    match f() {
        Ok(v) => {
            db.execute_batch("COMMIT")?;
            Ok(v)
        }
        Err(e) => {
            let _ = db.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

// ---------- kv: small internal state and settings (values are JSON) ----------

/// Settings → Agent: instructions every new project starts with.
pub const DEFAULT_INSTRUCTIONS_KEY: &str = "settings.defaultInstructions";
/// Settings → Agent: instructions added to every session.
pub const SESSION_INSTRUCTIONS_KEY: &str = "settings.sessionInstructions";
/// Permission mode a new session starts with.
pub const PERMISSION_KEY: &str = "settings.defaultPermission";

pub mod kv {
    use super::*;

    pub fn get<T: DeserializeOwned>(db: &Connection, key: &str) -> rusqlite::Result<Option<T>> {
        let raw: Option<String> = db.query_row("SELECT value FROM kv WHERE key = ?", [key], |r| r.get(0)).optional()?;
        Ok(raw.and_then(|v| serde_json::from_str(&v).ok()))
    }

    pub fn set<T: Serialize>(db: &Connection, key: &str, value: &T) -> rusqlite::Result<()> {
        let json = serde_json::to_string(value).unwrap_or_else(|_| "null".into());
        db.execute(
            "INSERT INTO kv (key, value, updated_at) VALUES (?, ?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            (key, json, now()),
        )?;
        Ok(())
    }
}

// ---------- projects ----------

pub mod projects {
    use super::*;

    fn group_ids_of(db: &Connection, project_id: &str) -> rusqlite::Result<Vec<String>> {
        let mut st = db.prepare_cached("SELECT group_id FROM group_members WHERE project_id = ?")?;
        st.query_map([project_id], |r| r.get(0))?.collect()
    }

    fn language(v: Option<String>) -> Option<Language> {
        v.filter(|s| !s.is_empty()).and_then(|s| serde_json::from_str(&s).ok())
    }

    fn non_empty(v: Option<String>) -> Option<String> {
        v.filter(|s| !s.is_empty())
    }

    fn from_row(db: &Connection, r: &Row) -> rusqlite::Result<Project> {
        let id: String = r.get("id")?;
        Ok(Project {
            group_ids: group_ids_of(db, &id)?,
            id,
            name: r.get("name")?,
            path: r.get("path")?,
            kind: if r.get::<_, String>("kind")? == "system" { ProjectKind::System } else { ProjectKind::Normal },
            description: r.get::<_, Option<String>>("description")?.unwrap_or_default(),
            instructions: r.get::<_, Option<String>>("instructions")?.unwrap_or_default(),
            git_provider: non_empty(r.get("git_provider")?).and_then(|s| GitProviderId::parse(&s)),
            repo_url: non_empty(r.get("repo_url")?),
            default_branch: non_empty(r.get("default_branch")?),
            language: language(r.get("language")?),
            missing: None,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
        })
    }

    fn query(db: &Connection, sql: &str, params: impl rusqlite::Params) -> rusqlite::Result<Vec<Project>> {
        let mut st = db.prepare(sql)?;
        let mut rows = st.query(params)?;
        let mut out = vec![];
        while let Some(r) = rows.next()? {
            out.push(from_row(db, r)?);
        }
        Ok(out)
    }

    fn one(db: &Connection, sql: &str, params: impl rusqlite::Params) -> rusqlite::Result<Option<Project>> {
        Ok(query(db, sql, params)?.into_iter().next())
    }

    pub fn list(db: &Connection) -> rusqlite::Result<Vec<Project>> {
        query(db, "SELECT * FROM projects ORDER BY updated_at DESC", [])
    }

    pub fn get(db: &Connection, id: &str) -> rusqlite::Result<Option<Project>> {
        one(db, "SELECT * FROM projects WHERE id = ?", [id])
    }

    pub fn by_path(db: &Connection, path: &str) -> rusqlite::Result<Option<Project>> {
        one(db, "SELECT * FROM projects WHERE path = ?", [path])
    }

    pub fn system(db: &Connection) -> rusqlite::Result<Option<Project>> {
        one(db, "SELECT * FROM projects WHERE kind = 'system' LIMIT 1", [])
    }

    pub fn create(db: &Connection, name: &str, path: &str, kind: ProjectKind) -> rusqlite::Result<Project> {
        let id = new_id();
        let t = now();
        // a new normal project starts with the default instructions from Settings → Agent
        let instructions = match kind {
            ProjectKind::Normal => kv::get::<String>(db, DEFAULT_INSTRUCTIONS_KEY)?.unwrap_or_default(),
            ProjectKind::System => String::new(),
        };
        db.execute(
            "INSERT INTO projects (id, name, path, kind, instructions, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
            (&id, name, path, kind.as_str(), instructions, &t, &t),
        )?;
        Ok(get(db, &id)?.expect("project just inserted"))
    }

    #[derive(Default)]
    pub struct Patch {
        pub name: Option<String>,
        pub description: Option<String>,
        pub instructions: Option<String>,
    }

    pub fn update(db: &Connection, id: &str, patch: Patch) -> rusqlite::Result<Option<Project>> {
        let Some(cur) = get(db, id)? else { return Ok(None) };
        db.execute(
            "UPDATE projects SET name = ?, description = ?, instructions = ?, updated_at = ? WHERE id = ?",
            (
                patch.name.unwrap_or(cur.name),
                patch.description.unwrap_or(cur.description),
                patch.instructions.unwrap_or(cur.instructions),
                now(),
                id,
            ),
        )?;
        get(db, id)
    }

    pub fn delete(db: &Connection, id: &str) -> rusqlite::Result<()> {
        db.execute("DELETE FROM projects WHERE id = ?", [id])?;
        Ok(())
    }

    /// Cached repository info (does not touch `updated_at`).
    pub fn set_repo_meta(
        db: &Connection,
        id: &str,
        git_provider: Option<GitProviderId>,
        repo_url: Option<&str>,
        default_branch: Option<&str>,
    ) -> rusqlite::Result<()> {
        let provider = git_provider.map(|p| serde_json::to_value(p).ok().and_then(|v| v.as_str().map(String::from)));
        db.execute(
            "UPDATE projects SET git_provider = ?, repo_url = ?, default_branch = ? WHERE id = ?",
            (provider.flatten(), repo_url, default_branch, id),
        )?;
        Ok(())
    }

    pub fn set_language(db: &Connection, id: &str, language: Option<&Language>) -> rusqlite::Result<()> {
        let json = language.map(|l| serde_json::to_string(l).expect("language serializes"));
        db.execute("UPDATE projects SET language = ? WHERE id = ?", (json, id))?;
        Ok(())
    }
}

// ---------- project groups ----------

pub mod groups {
    use super::*;

    fn from_row(db: &Connection, r: &Row) -> rusqlite::Result<ProjectGroup> {
        let id: String = r.get("id")?;
        let project_ids = {
            let mut st = db.prepare_cached("SELECT project_id FROM group_members WHERE group_id = ? ORDER BY position, rowid")?;
            st.query_map([&id], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?
        };
        Ok(ProjectGroup {
            id,
            name: r.get("name")?,
            description: r.get::<_, Option<String>>("description")?.unwrap_or_default(),
            instructions: r.get::<_, Option<String>>("instructions")?.unwrap_or_default(),
            folder: r.get::<_, Option<String>>("folder")?.filter(|s| !s.is_empty()),
            project_ids,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
        })
    }

    fn query(db: &Connection, sql: &str, params: impl rusqlite::Params) -> rusqlite::Result<Vec<ProjectGroup>> {
        let mut st = db.prepare(sql)?;
        let mut rows = st.query(params)?;
        let mut out = vec![];
        while let Some(r) = rows.next()? {
            out.push(from_row(db, r)?);
        }
        Ok(out)
    }

    pub fn list(db: &Connection) -> rusqlite::Result<Vec<ProjectGroup>> {
        query(db, "SELECT * FROM project_groups ORDER BY name COLLATE NOCASE", [])
    }

    pub fn get(db: &Connection, id: &str) -> rusqlite::Result<Option<ProjectGroup>> {
        Ok(query(db, "SELECT * FROM project_groups WHERE id = ?", [id])?.into_iter().next())
    }

    pub struct New {
        pub name: String,
        pub description: String,
        pub instructions: String,
        pub folder: Option<String>,
        pub project_ids: Vec<String>,
    }

    pub fn create(db: &Connection, input: New) -> rusqlite::Result<ProjectGroup> {
        let id = new_id();
        let t = now();
        tx(db, || {
            db.execute(
                "INSERT INTO project_groups (id, name, description, instructions, folder, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                (&id, &input.name, &input.description, &input.instructions, &input.folder, &t, &t),
            )?;
            set_members(db, &id, &input.project_ids)
        })?;
        Ok(get(db, &id)?.expect("group just inserted"))
    }

    /// `folder`: `None` keeps the current value, `Some(None)` clears it.
    #[derive(Default)]
    pub struct Patch {
        pub name: Option<String>,
        pub description: Option<String>,
        pub instructions: Option<String>,
        pub folder: Option<Option<String>>,
        pub project_ids: Option<Vec<String>>,
    }

    pub fn update(db: &Connection, id: &str, patch: Patch) -> rusqlite::Result<Option<ProjectGroup>> {
        let Some(cur) = get(db, id)? else { return Ok(None) };
        tx(db, || {
            db.execute(
                "UPDATE project_groups SET name = ?, description = ?, instructions = ?, folder = ?, updated_at = ? WHERE id = ?",
                (
                    patch.name.unwrap_or(cur.name),
                    patch.description.unwrap_or(cur.description),
                    patch.instructions.unwrap_or(cur.instructions),
                    patch.folder.unwrap_or(cur.folder),
                    now(),
                    id,
                ),
            )?;
            match &patch.project_ids {
                Some(ids) => set_members(db, id, ids),
                None => Ok(()),
            }
        })?;
        get(db, id)
    }

    /// Inside a transaction: replace the group's members (order as in the list).
    fn set_members(db: &Connection, id: &str, project_ids: &[String]) -> rusqlite::Result<()> {
        db.execute("DELETE FROM group_members WHERE group_id = ?", [id])?;
        for (i, pid) in project_ids.iter().enumerate() {
            db.execute("INSERT INTO group_members (group_id, project_id, position) VALUES (?, ?, ?)", (id, pid, i as i64))?;
        }
        Ok(())
    }

    pub fn delete(db: &Connection, id: &str) -> rusqlite::Result<()> {
        db.execute("DELETE FROM project_groups WHERE id = ?", [id])?;
        Ok(())
    }
}

// ---------- workspaces ----------

pub mod workspaces {
    use super::*;

    /// A workspace row (capabilities come from the provider, not the database).
    #[derive(Debug, Clone)]
    pub struct WorkspaceRow {
        pub id: String,
        pub scope: WorkspaceScopeKind,
        pub scope_id: String,
        pub provider: String,
        pub strategy: WorkspaceStrategy,
        pub path: String,
        pub status: WorkspaceStatus,
        pub status_detail: Option<String>,
        pub created_at: String,
        pub updated_at: String,
    }

    fn strategy_str(s: WorkspaceStrategy) -> &'static str {
        match s {
            WorkspaceStrategy::Single => "single",
            WorkspaceStrategy::Multi => "multi",
        }
    }

    fn status_str(s: WorkspaceStatus) -> &'static str {
        match s {
            WorkspaceStatus::Ready => "ready",
            WorkspaceStatus::Missing => "missing",
            WorkspaceStatus::Error => "error",
        }
    }

    fn from_row(r: &Row) -> rusqlite::Result<WorkspaceRow> {
        Ok(WorkspaceRow {
            id: r.get("id")?,
            scope: if r.get::<_, String>("scope")? == "group" { WorkspaceScopeKind::Group } else { WorkspaceScopeKind::Project },
            scope_id: r.get("scope_id")?,
            provider: r.get("provider")?,
            strategy: if r.get::<_, String>("strategy")? == "multi" { WorkspaceStrategy::Multi } else { WorkspaceStrategy::Single },
            path: r.get("path")?,
            status: match r.get::<_, String>("status")?.as_str() {
                "ready" => WorkspaceStatus::Ready,
                "missing" => WorkspaceStatus::Missing,
                _ => WorkspaceStatus::Error,
            },
            status_detail: r.get("status_detail")?,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
        })
    }

    pub fn find(db: &Connection, scope: WorkspaceScopeKind, scope_id: &str, provider: &str) -> rusqlite::Result<Option<WorkspaceRow>> {
        db.query_row(
            "SELECT * FROM workspaces WHERE scope = ? AND scope_id = ? AND provider = ?",
            (scope.as_str(), scope_id, provider),
            from_row,
        )
        .optional()
    }

    pub fn get(db: &Connection, id: &str) -> rusqlite::Result<Option<WorkspaceRow>> {
        db.query_row("SELECT * FROM workspaces WHERE id = ?", [id], from_row).optional()
    }

    pub struct Upsert<'a> {
        pub scope: WorkspaceScopeKind,
        pub scope_id: &'a str,
        pub provider: &'a str,
        pub strategy: WorkspaceStrategy,
        pub path: &'a str,
        pub status: WorkspaceStatus,
        pub status_detail: Option<&'a str>,
    }

    pub fn upsert(db: &Connection, w: Upsert) -> rusqlite::Result<WorkspaceRow> {
        let t = now();
        let id = match find(db, w.scope, w.scope_id, w.provider)? {
            Some(cur) => {
                db.execute(
                    "UPDATE workspaces SET strategy = ?, path = ?, status = ?, status_detail = ?, updated_at = ? WHERE id = ?",
                    (strategy_str(w.strategy), w.path, status_str(w.status), w.status_detail, &t, &cur.id),
                )?;
                cur.id
            }
            None => {
                let id = new_id();
                db.execute(
                    "INSERT INTO workspaces (id, scope, scope_id, provider, strategy, path, status, status_detail, created_at, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    (
                        &id,
                        w.scope.as_str(),
                        w.scope_id,
                        w.provider,
                        strategy_str(w.strategy),
                        w.path,
                        status_str(w.status),
                        w.status_detail,
                        &t,
                        &t,
                    ),
                )?;
                id
            }
        };
        Ok(get(db, &id)?.expect("workspace just written"))
    }

    pub fn delete_scope(db: &Connection, scope: WorkspaceScopeKind, scope_id: &str) -> rusqlite::Result<()> {
        db.execute("DELETE FROM workspaces WHERE scope = ? AND scope_id = ?", (scope.as_str(), scope_id))?;
        Ok(())
    }
}
