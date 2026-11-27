import type { DatabaseSync } from 'node:sqlite';

/**
 * Versioned schema and data migrations. The database version is PRAGMA user_version;
 * on start every migration above it runs once, in order, each in its own transaction
 * (a failed step leaves the database at the previous version). Add a step to the end of
 * MIGRATIONS — never edit or reorder a released one.
 */

export interface Migration {
  version: number;
  name: string;
  up(db: DatabaseSync): void;
}

/**
 * v1 — the schema as of the switch to versioned migrations. Idempotent on purpose: databases
 * created before versioning (user_version 0) already have some of these tables and columns.
 */
function baseline(db: DatabaseSync) {
  db.exec(`
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
-- many-to-many: a project can belong to several groups; the project record is not copied
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
-- project run commands (Run); not written to the repository
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
-- project secrets: the value is encrypted (AES-256-GCM, secrets.ts)
CREATE TABLE IF NOT EXISTS secrets (
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  value TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (project_id, name)
);
-- git logins per host for the hub's own git runs: the secret is encrypted (git-auth.ts)
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
`);
  const addColumn = (table: string, column: string, ddl: string) => {
    const cols = db.prepare(`PRAGMA table_info(${table})`).all().map((r) => r.name as string);
    if (!cols.includes(column)) db.exec(`ALTER TABLE ${table} ADD COLUMN ${ddl}`);
  };
  // proactive alerts and external events were removed
  db.exec('DROP TABLE IF EXISTS alerts; DROP TABLE IF EXISTS hub_events;');
  addColumn('projects', 'kind', "kind TEXT NOT NULL DEFAULT 'normal'");
  addColumn('sessions', 'cwd', 'cwd TEXT');
  addColumn('sessions', 'handoff', 'handoff TEXT');
  addColumn('projects', 'description', "description TEXT NOT NULL DEFAULT ''");
  addColumn('projects', 'instructions', "instructions TEXT NOT NULL DEFAULT ''");
  addColumn('projects', 'git_provider', 'git_provider TEXT');
  addColumn('projects', 'repo_url', 'repo_url TEXT');
  addColumn('projects', 'default_branch', 'default_branch TEXT');
  addColumn('project_groups', 'folder', 'folder TEXT');
  addColumn('projects', 'runtime', 'runtime TEXT');
  addColumn('projects', 'language', 'language TEXT');
  addColumn('run_configs', 'docker', 'docker INTEGER NOT NULL DEFAULT 0');
  addColumn('sessions', 'group_id', 'group_id TEXT REFERENCES project_groups(id) ON DELETE CASCADE');
  addColumn('sessions', 'workspace_id', 'workspace_id TEXT');
  addColumn('sessions', 'archived_at', 'archived_at TEXT');
}

/**
 * Migration: a group used to be a projects row (kind = 'group'), with members
 * in project_roots by path. Now a group is project_groups (same id),
 * members are group_members, group sessions are sessions.group_id.
 */
export function migrateLegacyGroups(db: DatabaseSync) {
  const legacy = db.prepare("SELECT * FROM projects WHERE kind = 'group'").all();
  for (const g of legacy) {
    const id = g.id as string;
      db.prepare('INSERT OR IGNORE INTO project_groups (id, name, created_at, updated_at) VALUES (?, ?, ?, ?)').run(
        id,
        g.name as string,
        g.created_at as string,
        g.updated_at as string,
      );
      const roots = db.prepare('SELECT path, position FROM project_roots WHERE project_id = ? ORDER BY position').all(id);
      for (const root of roots) {
        const m = db.prepare("SELECT id FROM projects WHERE path = ? AND kind = 'normal'").get(root.path as string);
        if (m) db.prepare('INSERT OR IGNORE INTO group_members (group_id, project_id, position) VALUES (?, ?, ?)').run(id, m.id as string, Number(root.position));
      }
      db.prepare('UPDATE sessions SET group_id = ?, project_id = NULL WHERE project_id = ?').run(id, id);
      db.prepare('DELETE FROM project_roots WHERE project_id = ?').run(id);
      db.prepare('DELETE FROM projects WHERE id = ?').run(id);
  }
}

/**
 * v3 — projects used to be imported automatically from the projects folder. Remove such
 * records if nothing is attached to them (no sessions, groups, description or instructions).
 * Files are untouched. Skipped where the pre-versioning one-time run already did it.
 */
export function pruneAutoImported(db: DatabaseSync) {
  if (db.prepare("SELECT 1 FROM kv WHERE key = 'migration.explicitProjects'").get()) return;
  db.exec(`DELETE FROM projects
    WHERE kind = 'normal' AND description = '' AND instructions = ''
      AND id NOT IN (SELECT project_id FROM group_members)
      AND id NOT IN (SELECT project_id FROM sessions WHERE project_id IS NOT NULL)`);
}

/**
 * v4 — user accounts (username + argon2id password hash) and their sign-in sessions (only a
 * SHA-256 of the session token is stored). Sign-in itself is implemented by the Rust server.
 */
export function userAccounts(db: DatabaseSync) {
  db.exec(`
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
`);
}

/**
 * v5 — user permissions: a role (`admin` — everything, `member` — what `permissions`, a JSON
 * object, allows) and the account that started each session (null for older sessions and
 * hubs without accounts). Existing accounts stay admins.
 */
export function userPermissions(db: DatabaseSync) {
  db.exec(`
ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'admin';
ALTER TABLE users ADD COLUMN permissions TEXT;
ALTER TABLE sessions ADD COLUMN created_by TEXT;
`);
}

/**
 * v6 — SSH connections: servers the hub reaches with keys it holds (the private key encrypted
 * like project secrets), attached to projects and groups with a note on what they are for.
 */
export function sshConnections(db: DatabaseSync) {
  db.exec(`
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
`);
}

/**
 * v7 — hubness is single-user: one account (the first one created) stays, any other
 * account goes with its sessions. Roles and permissions (v5 columns) are no longer read.
 */
export function singleAccount(db: DatabaseSync) {
  db.exec(`
DELETE FROM auth_sessions WHERE user_id NOT IN (SELECT id FROM users ORDER BY created_at, rowid LIMIT 1);
DELETE FROM users WHERE id NOT IN (SELECT id FROM users ORDER BY created_at, rowid LIMIT 1);
`);
}

/**
 * v8 — git logins remember their last check: whether the host accepted the login, who it is
 * there, and the account's picture (kept in the hub: a private host serves it only with the
 * login, and the browser has no business asking other servers).
 */
export function gitLoginChecks(db: DatabaseSync) {
  db.exec(`
ALTER TABLE git_credentials ADD COLUMN check_status TEXT NOT NULL DEFAULT '';
ALTER TABLE git_credentials ADD COLUMN checked_at TEXT;
ALTER TABLE git_credentials ADD COLUMN check_error TEXT NOT NULL DEFAULT '';
ALTER TABLE git_credentials ADD COLUMN account TEXT NOT NULL DEFAULT '';
ALTER TABLE git_credentials ADD COLUMN avatar BLOB;
ALTER TABLE git_credentials ADD COLUMN avatar_type TEXT NOT NULL DEFAULT '';
`);
}

export const MIGRATIONS: Migration[] = [
  { version: 1, name: 'baseline schema', up: baseline },
  { version: 2, name: 'legacy groups (projects.kind = group) → project_groups', up: migrateLegacyGroups },
  { version: 3, name: 'drop auto-imported projects', up: pruneAutoImported },
  { version: 4, name: 'user accounts and their sign-in sessions', up: userAccounts },
  { version: 5, name: 'user roles, permissions and session owners', up: userPermissions },
  { version: 6, name: 'SSH connections and their project / group bindings', up: sshConnections },
  { version: 7, name: 'one account: the first one stays', up: singleAccount },
  { version: 8, name: 'git logins: the last check and the account picture', up: gitLoginChecks },
];

export const schemaVersion = (db: DatabaseSync) => Number(db.prepare('PRAGMA user_version').get()?.user_version ?? 0);

/** Bring the database up to the latest version. Returns the migrations applied. */
export function migrate(db: DatabaseSync, list: Migration[] = MIGRATIONS): Migration[] {
  const applied: Migration[] = [];
  for (const m of [...list].sort((a, b) => a.version - b.version)) {
    if (m.version <= schemaVersion(db)) continue;
    db.exec('BEGIN');
    try {
      m.up(db);
      db.exec(`PRAGMA user_version = ${m.version}`);
      db.exec('COMMIT');
    } catch (e) {
      db.exec('ROLLBACK');
      throw new Error(`Migration ${m.version} (${m.name}) failed: ${(e as Error).message}`);
    }
    applied.push(m);
  }
  return applied;
}
