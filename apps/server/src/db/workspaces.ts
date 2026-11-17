import { randomUUID } from 'node:crypto';
import type { Workspace } from '@hubness/shared';
import { db, now, type Row } from './connection.ts';

// ---------- workspaces ----------

type WorkspaceRow = Omit<Workspace, 'capabilities'>;

const toWorkspace = (r: Row): WorkspaceRow => ({
  id: r.id as string,
  scope: r.scope as Workspace['scope'],
  scopeId: r.scope_id as string,
  provider: r.provider as string,
  strategy: r.strategy as Workspace['strategy'],
  path: r.path as string,
  status: r.status as Workspace['status'],
  statusDetail: (r.status_detail as string) ?? null,
  createdAt: r.created_at as string,
  updatedAt: r.updated_at as string,
});

export const workspaces = {
  find(scope: Workspace['scope'], scopeId: string, provider: string): WorkspaceRow | undefined {
    const r = db.prepare('SELECT * FROM workspaces WHERE scope = ? AND scope_id = ? AND provider = ?').get(scope, scopeId, provider);
    return r ? toWorkspace(r) : undefined;
  },
  get(id: string): WorkspaceRow | undefined {
    const r = db.prepare('SELECT * FROM workspaces WHERE id = ?').get(id);
    return r ? toWorkspace(r) : undefined;
  },
  upsert(w: Pick<WorkspaceRow, 'scope' | 'scopeId' | 'provider' | 'strategy' | 'path' | 'status' | 'statusDetail'>): WorkspaceRow {
    const cur = workspaces.find(w.scope, w.scopeId, w.provider);
    const t = now();
    if (cur) {
      db.prepare('UPDATE workspaces SET strategy = ?, path = ?, status = ?, status_detail = ?, updated_at = ? WHERE id = ?').run(
        w.strategy,
        w.path,
        w.status,
        w.statusDetail,
        t,
        cur.id,
      );
      return workspaces.get(cur.id)!;
    }
    const id = randomUUID();
    db.prepare(
      `INSERT INTO workspaces (id, scope, scope_id, provider, strategy, path, status, status_detail, created_at, updated_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
    ).run(id, w.scope, w.scopeId, w.provider, w.strategy, w.path, w.status, w.statusDetail, t, t);
    return workspaces.get(id)!;
  },
  deleteScope(scope: Workspace['scope'], scopeId: string) {
    db.prepare('DELETE FROM workspaces WHERE scope = ? AND scope_id = ?').run(scope, scopeId);
  },
};
