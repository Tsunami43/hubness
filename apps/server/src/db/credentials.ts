import { db, now } from './connection.ts';

// ---------- secrets (values are already encrypted — see secrets.ts) ----------

export const secretRows = {
  list(projectId: string): { name: string; value: string; updatedAt: string }[] {
    return db
      .prepare('SELECT name, value, updated_at FROM secrets WHERE project_id = ? ORDER BY name')
      .all(projectId)
      .map((r) => ({ name: r.name as string, value: r.value as string, updatedAt: r.updated_at as string }));
  },
  set(projectId: string, name: string, value: string) {
    db.prepare(
      `INSERT INTO secrets (project_id, name, value, updated_at) VALUES (?, ?, ?, ?)
       ON CONFLICT(project_id, name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at`,
    ).run(projectId, name, value, now());
  },
  delete(projectId: string, name: string) {
    db.prepare('DELETE FROM secrets WHERE project_id = ? AND name = ?').run(projectId, name);
  },
};

// ---------- git credentials (secret is already encrypted — see git-auth.ts) ----------

export interface GitCredentialRow {
  host: string;
  username: string;
  kind: string;
  secret: string;
  updatedAt: string;
}

export const gitCredentialRows = {
  list(): GitCredentialRow[] {
    return db
      .prepare('SELECT host, username, kind, secret, updated_at FROM git_credentials ORDER BY host')
      .all()
      .map((r) => ({ host: r.host as string, username: r.username as string, kind: r.kind as string, secret: r.secret as string, updatedAt: r.updated_at as string }));
  },
  set(host: string, username: string, kind: string, secret: string) {
    db.prepare(
      `INSERT INTO git_credentials (host, username, kind, secret, updated_at) VALUES (?, ?, ?, ?, ?)
       ON CONFLICT(host) DO UPDATE SET username = excluded.username, kind = excluded.kind, secret = excluded.secret, updated_at = excluded.updated_at`,
    ).run(host, username, kind, secret, now());
  },
  delete(host: string) {
    db.prepare('DELETE FROM git_credentials WHERE host = ?').run(host);
  },
};
