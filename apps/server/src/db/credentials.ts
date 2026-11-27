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
  /** '' — never checked; 'ok' | 'failed' | 'unknown'. */
  checkStatus: string;
  checkedAt: string | null;
  checkError: string;
  account: string;
  hasAvatar: boolean;
}

export interface GitCheckResult {
  status: 'ok' | 'failed' | 'unknown';
  error: string;
  account: string;
  /** undefined — keep the picture there is; null — there is none. */
  avatar?: { type: string; data: Uint8Array } | null;
}

export const gitCredentialRows = {
  list(): GitCredentialRow[] {
    return db
      .prepare('SELECT host, username, kind, secret, updated_at, check_status, checked_at, check_error, account, avatar IS NOT NULL AS has_avatar FROM git_credentials ORDER BY host')
      .all()
      .map((r) => ({
        host: r.host as string,
        username: r.username as string,
        kind: r.kind as string,
        secret: r.secret as string,
        updatedAt: r.updated_at as string,
        checkStatus: r.check_status as string,
        checkedAt: (r.checked_at as string | null) ?? null,
        checkError: r.check_error as string,
        account: r.account as string,
        hasAvatar: Boolean(r.has_avatar),
      }));
  },
  /** A new login is a new question: what the last check said no longer holds. */
  set(host: string, username: string, kind: string, secret: string) {
    db.prepare(
      `INSERT INTO git_credentials (host, username, kind, secret, updated_at) VALUES (?, ?, ?, ?, ?)
       ON CONFLICT(host) DO UPDATE SET username = excluded.username, kind = excluded.kind, secret = excluded.secret, updated_at = excluded.updated_at,
         check_status = '', checked_at = NULL, check_error = '', account = '', avatar = NULL, avatar_type = ''`,
    ).run(host, username, kind, secret, now());
  },
  delete(host: string) {
    db.prepare('DELETE FROM git_credentials WHERE host = ?').run(host);
  },
  checked(host: string, r: GitCheckResult) {
    // who it is stays known when the host refuses or can't be asked: it is still that account
    db.prepare("UPDATE git_credentials SET check_status = ?, checked_at = ?, check_error = ?, account = CASE WHEN ? != '' THEN ? ELSE account END WHERE host = ?").run(
      r.status,
      now(),
      r.error,
      r.account,
      r.account,
      host,
    );
    if (r.avatar !== undefined) db.prepare('UPDATE git_credentials SET avatar = ?, avatar_type = ? WHERE host = ?').run(r.avatar?.data ?? null, r.avatar?.type ?? '', host);
  },
  avatar(host: string): { type: string; data: Uint8Array } | null {
    const r = db.prepare('SELECT avatar, avatar_type FROM git_credentials WHERE host = ?').get(host) as { avatar: Uint8Array | null; avatar_type: string } | undefined;
    return r?.avatar ? { type: r.avatar_type, data: r.avatar } : null;
  },
};
