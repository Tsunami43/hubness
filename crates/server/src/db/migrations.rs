//! Versioned schema and data migrations — a port of `apps/server/src/db/migrations.ts`.
//!
//! Both servers share the database file, so the list must stay identical to the TypeScript
//! one: same versions, same effects. The version is `PRAGMA user_version`; every migration
//! above it runs once, in order, each in its own transaction. Never edit or reorder a
//! released migration — append a new one (in both servers).

use rusqlite::{Connection, OptionalExtension};

pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub up: fn(&Connection) -> rusqlite::Result<()>,
}

pub const MIGRATIONS: &[Migration] = &[
    Migration { version: 1, name: "baseline schema", up: baseline },
    Migration { version: 2, name: "legacy groups (projects.kind = group) → project_groups", up: migrate_legacy_groups },
    Migration { version: 3, name: "drop auto-imported projects", up: prune_auto_imported },
    Migration { version: 4, name: "user accounts and their sign-in sessions", up: user_accounts },
    Migration { version: 5, name: "user roles, permissions and session owners", up: user_permissions },
    Migration { version: 6, name: "SSH connections and their project / group bindings", up: ssh_connections },
    Migration { version: 7, name: "one account: the first one stays", up: single_account },
];

const BASELINE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  path TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
  id TEXT PRIMARY KEY,
  project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
  card_id TEXT,
  title TEXT NOT NULL,
  agent TEXT NOT NULL,
  agent_session_id TEXT,
  status TEXT NOT NULL DEFAULT 'idle',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS sessions_project ON sessions(project_id, updated_at DESC);
CREATE TABLE IF NOT EXISTS events (
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  uid TEXT NOT NULL,
  seq INTEGER NOT NULL,
  data TEXT NOT NULL,
  PRIMARY KEY (session_id, uid)
);
CREATE INDEX IF NOT EXISTS events_seq ON events(session_id, seq);
CREATE TABLE IF NOT EXISTS cards (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  col TEXT NOT NULL DEFAULT 'todo',
  position REAL NOT NULL DEFAULT 0,
  session_id TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS cards_project ON cards(project_id, col, position);
CREATE TABLE IF NOT EXISTS project_roots (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  name TEXT NOT NULL,
  position INTEGER NOT NULL DEFAULT 0,
  UNIQUE (project_id, path)
);
CREATE TABLE IF NOT EXISTS project_groups (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  instructions TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS group_members (
  group_id TEXT NOT NULL REFERENCES project_groups(id) ON DELETE CASCADE,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  position INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (group_id, project_id)
);
CREATE INDEX IF NOT EXISTS group_members_project ON group_members(project_id);
CREATE TABLE IF NOT EXISTS workspaces (
  id TEXT PRIMARY KEY,
  scope TEXT NOT NULL,
  scope_id TEXT NOT NULL,
  provider TEXT NOT NULL,
  strategy TEXT NOT NULL,
  path TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'ready',
  status_detail TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE (scope, scope_id, provider)
);
CREATE TABLE IF NOT EXISTS run_configs (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  command TEXT NOT NULL,
  port INTEGER,
  is_primary INTEGER NOT NULL DEFAULT 0,
  position INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS run_configs_project ON run_configs(project_id, position);
CREATE TABLE IF NOT EXISTS secrets (
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  value TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (project_id, name)
);
CREATE TABLE IF NOT EXISTS git_credentials (
  host TEXT PRIMARY KEY,
  username TEXT NOT NULL,
  kind TEXT NOT NULL,
  secret TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS kv (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
DROP TABLE IF EXISTS alerts;
DROP TABLE IF EXISTS hub_events;
"#;

/// v1 — the schema as of the switch to versioned migrations. Idempotent on purpose:
/// databases created before versioning already have some of these tables and columns.
fn baseline(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(BASELINE_SQL)?;
    let columns: &[(&str, &str, &str)] = &[
        ("projects", "kind", "kind TEXT NOT NULL DEFAULT 'normal'"),
        ("sessions", "cwd", "cwd TEXT"),
        ("sessions", "handoff", "handoff TEXT"),
        ("projects", "description", "description TEXT NOT NULL DEFAULT ''"),
        ("projects", "instructions", "instructions TEXT NOT NULL DEFAULT ''"),
        ("projects", "git_provider", "git_provider TEXT"),
        ("projects", "repo_url", "repo_url TEXT"),
        ("projects", "default_branch", "default_branch TEXT"),
        ("project_groups", "folder", "folder TEXT"),
        ("projects", "runtime", "runtime TEXT"),
        ("projects", "language", "language TEXT"),
        ("run_configs", "docker", "docker INTEGER NOT NULL DEFAULT 0"),
        ("sessions", "group_id", "group_id TEXT REFERENCES project_groups(id) ON DELETE CASCADE"),
        ("sessions", "workspace_id", "workspace_id TEXT"),
        ("sessions", "archived_at", "archived_at TEXT"),
    ];
    for (table, column, ddl) in columns {
        if !has_column(db, table, column)? {
            db.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {ddl}"))?;
        }
    }
    Ok(())
}

fn has_column(db: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut st = db.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = st.query_map([], |r| r.get::<_, String>("name"))?.collect::<Result<Vec<_>, _>>()?;
    Ok(names.iter().any(|n| n == column))
}

/// v2 — a group used to be a projects row (kind = 'group') with members in project_roots
/// by path. Now a group is project_groups (same id), members are group_members, group
/// sessions are sessions.group_id.
fn migrate_legacy_groups(db: &Connection) -> rusqlite::Result<()> {
    let legacy: Vec<(String, String, String, String)> = {
        let mut st = db.prepare("SELECT id, name, created_at, updated_at FROM projects WHERE kind = 'group'")?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<_, _>>()?
    };
    for (id, name, created, updated) in legacy {
        db.execute(
            "INSERT OR IGNORE INTO project_groups (id, name, created_at, updated_at) VALUES (?, ?, ?, ?)",
            (&id, &name, &created, &updated),
        )?;
        let roots: Vec<(String, i64)> = {
            let mut st = db.prepare("SELECT path, position FROM project_roots WHERE project_id = ? ORDER BY position")?;
            st.query_map([&id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?
        };
        for (path, position) in roots {
            let member: Option<String> =
                db.query_row("SELECT id FROM projects WHERE path = ? AND kind = 'normal'", [&path], |r| r.get(0)).optional()?;
            if let Some(pid) = member {
                db.execute("INSERT OR IGNORE INTO group_members (group_id, project_id, position) VALUES (?, ?, ?)", (&id, &pid, position))?;
            }
        }
        db.execute("UPDATE sessions SET group_id = ?, project_id = NULL WHERE project_id = ?", [&id, &id])?;
        db.execute("DELETE FROM project_roots WHERE project_id = ?", [&id])?;
        db.execute("DELETE FROM projects WHERE id = ?", [&id])?;
    }
    Ok(())
}

/// v3 — projects used to be imported automatically from the projects folder. Remove such
/// records if nothing is attached to them. Files are untouched. Skipped where the
/// pre-versioning one-time run already did it.
fn prune_auto_imported(db: &Connection) -> rusqlite::Result<()> {
    let done: Option<i64> = db.query_row("SELECT 1 FROM kv WHERE key = 'migration.explicitProjects'", [], |r| r.get(0)).optional()?;
    if done.is_some() {
        return Ok(());
    }
    db.execute_batch(
        "DELETE FROM projects
         WHERE kind = 'normal' AND description = '' AND instructions = ''
           AND id NOT IN (SELECT project_id FROM group_members)
           AND id NOT IN (SELECT project_id FROM sessions WHERE project_id IS NOT NULL)",
    )
}

/// v4 — user accounts (username + argon2id password hash) and their sign-in sessions
/// (only a SHA-256 of the session token is stored).
fn user_accounts(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(USERS_SQL)
}

const USERS_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS users (
  id TEXT PRIMARY KEY,
  username TEXT NOT NULL COLLATE NOCASE UNIQUE,
  password_hash TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS auth_sessions (
  token_hash TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at TEXT NOT NULL,
  last_seen TEXT NOT NULL,
  expires TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS auth_sessions_user ON auth_sessions(user_id);
"#;

/// v5 — user permissions: a role (`admin` — everything, `member` — what `permissions`, a JSON
/// object, allows) and the account that started each session (null for older sessions and
/// hubs without accounts). Existing accounts stay admins.
fn user_permissions(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'admin';
ALTER TABLE users ADD COLUMN permissions TEXT;
ALTER TABLE sessions ADD COLUMN created_by TEXT;",
    )
}

/// v6 — SSH connections: servers the hub reaches with keys it holds (the private key
/// encrypted like project secrets), attached to projects and groups with a note on what they
/// are for.
fn ssh_connections(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS ssh_connections (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL COLLATE NOCASE UNIQUE,
  host TEXT NOT NULL,
  port INTEGER NOT NULL DEFAULT 22,
  username TEXT NOT NULL,
  environment TEXT NOT NULL DEFAULT '',
  description TEXT NOT NULL DEFAULT '',
  public_key TEXT NOT NULL,
  private_key TEXT NOT NULL,
  host_keys TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS ssh_bindings (
  connection_id TEXT NOT NULL REFERENCES ssh_connections(id) ON DELETE CASCADE,
  project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
  group_id TEXT REFERENCES project_groups(id) ON DELETE CASCADE,
  note TEXT NOT NULL DEFAULT '',
  position INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS ssh_bindings_project ON ssh_bindings(project_id);
CREATE INDEX IF NOT EXISTS ssh_bindings_group ON ssh_bindings(group_id);
"#,
    )
}

/// v7 — hubness is single-user: one account (the first one created) stays, any other
/// account goes with its sessions. Roles and permissions (v5 columns) are no longer read.
fn single_account(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "
DELETE FROM auth_sessions WHERE user_id NOT IN (SELECT id FROM users ORDER BY created_at, rowid LIMIT 1);
DELETE FROM users WHERE id NOT IN (SELECT id FROM users ORDER BY created_at, rowid LIMIT 1);
",
    )
}

pub fn schema_version(db: &Connection) -> rusqlite::Result<i64> {
    db.query_row("PRAGMA user_version", [], |r| r.get(0))
}

/// Bring the database up to the latest version. Returns the versions applied.
pub fn migrate(db: &Connection, list: &[Migration]) -> anyhow::Result<Vec<i64>> {
    let mut sorted: Vec<&Migration> = list.iter().collect();
    sorted.sort_by_key(|m| m.version);
    let mut applied = vec![];
    for m in sorted {
        if m.version <= schema_version(db)? {
            continue;
        }
        db.execute_batch("BEGIN")?;
        let step = (m.up)(db).and_then(|_| db.execute_batch(&format!("PRAGMA user_version = {}", m.version)));
        match step {
            Ok(()) => db.execute_batch("COMMIT")?,
            Err(e) => {
                db.execute_batch("ROLLBACK")?;
                anyhow::bail!("Migration {} ({}) failed: {e}", m.version, m.name);
            }
        }
        applied.push(m.version);
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_database_reaches_the_latest_version_once() {
        let db = Connection::open_in_memory().unwrap();
        assert_eq!(migrate(&db, MIGRATIONS).unwrap(), vec![1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(schema_version(&db).unwrap(), 7);
        assert!(migrate(&db, MIGRATIONS).unwrap().is_empty());
        assert!(has_column(&db, "sessions", "archived_at").unwrap());
    }

    #[test]
    fn only_the_first_account_stays() {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db, &MIGRATIONS[..6]).unwrap();
        db.execute_batch(
            "INSERT INTO users (id, username, password_hash, created_at) VALUES
               ('b', 'bob', 'h', '2026-02-01T00:00:00Z'), ('a', 'ann', 'h', '2026-01-01T00:00:00Z'), ('c', 'cy', 'h', '2026-03-01T00:00:00Z');
             INSERT INTO auth_sessions (token_hash, user_id, created_at, last_seen, expires) VALUES ('t1', 'a', 't', 't', 't'), ('t2', 'b', 't', 't', 't');",
        )
        .unwrap();
        assert_eq!(migrate(&db, MIGRATIONS).unwrap(), vec![7]);
        let ids: Vec<String> =
            db.prepare("SELECT id FROM users").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        assert_eq!(ids, ["a"]);
        let sessions: i64 = db.query_row("SELECT COUNT(*) FROM auth_sessions", [], |r| r.get(0)).unwrap();
        assert_eq!(sessions, 1);
    }

    #[test]
    fn legacy_group_rows_become_project_groups() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL UNIQUE,
               created_at TEXT NOT NULL, updated_at TEXT NOT NULL, kind TEXT NOT NULL DEFAULT 'normal',
               description TEXT NOT NULL DEFAULT 'x');
             INSERT INTO projects VALUES ('p1', 'api', '/r/api', 't', 't', 'normal', 'keep');
             INSERT INTO projects VALUES ('g1', 'Platform', '/r/g', 't', 't', 'group', '');
             CREATE TABLE project_roots (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, path TEXT NOT NULL,
               name TEXT NOT NULL, position INTEGER NOT NULL DEFAULT 0);
             INSERT INTO project_roots VALUES ('r1', 'g1', '/r/api', 'api', 0);",
        )
        .unwrap();
        migrate(&db, MIGRATIONS).unwrap();
        let member: String = db.query_row("SELECT project_id FROM group_members WHERE group_id = 'g1'", [], |r| r.get(0)).unwrap();
        assert_eq!(member, "p1");
        let left: i64 = db.query_row("SELECT COUNT(*) FROM projects WHERE id = 'g1'", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn a_failed_step_rolls_back_and_keeps_the_version() {
        fn broken(db: &Connection) -> rusqlite::Result<()> {
            db.execute_batch("CREATE TABLE t (x); SELECT * FROM nope;")
        }
        let db = Connection::open_in_memory().unwrap();
        let list = [Migration { version: 1, name: "broken", up: broken }];
        assert!(migrate(&db, &list).is_err());
        assert_eq!(schema_version(&db).unwrap(), 0);
        assert!(db.prepare("SELECT * FROM t").is_err(), "the table was rolled back");
    }
}
