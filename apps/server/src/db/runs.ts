import { randomUUID } from 'node:crypto';
import type { RunConfig } from '@hubness/shared';
import { db, type Row } from './connection.ts';

// ---------- run commands ----------

const toRun = (r: Row): RunConfig => ({
  id: r.id as string,
  projectId: r.project_id as string,
  name: r.name as string,
  command: r.command as string,
  port: r.port == null ? null : Number(r.port),
  primary: Boolean(r.is_primary),
  docker: Boolean(r.docker),
  position: Number(r.position),
});

export const runConfigs = {
  list(projectId: string): RunConfig[] {
    return db.prepare('SELECT * FROM run_configs WHERE project_id = ? ORDER BY position, rowid').all(projectId).map(toRun);
  },
  get(id: string): RunConfig | undefined {
    const r = db.prepare('SELECT * FROM run_configs WHERE id = ?').get(id);
    return r ? toRun(r) : undefined;
  },
  create(projectId: string, input: { name: string; command: string; port: number | null; primary: boolean; docker?: boolean }): RunConfig {
    const id = randomUUID();
    const pos = Number(db.prepare('SELECT COALESCE(MAX(position), -1) + 1 AS p FROM run_configs WHERE project_id = ?').get(projectId)!.p);
    const first = !runConfigs.list(projectId).length;
    db.prepare('INSERT INTO run_configs (id, project_id, name, command, port, is_primary, docker, position) VALUES (?, ?, ?, ?, ?, ?, ?, ?)').run(
      id,
      projectId,
      input.name,
      input.command,
      input.port,
      0,
      input.docker ? 1 : 0,
      pos,
    );
    if (input.primary || first) runConfigs.setPrimary(projectId, id);
    return runConfigs.get(id)!;
  },
  update(id: string, patch: { name?: string; command?: string; port?: number | null; docker?: boolean }): RunConfig | undefined {
    const cur = runConfigs.get(id);
    if (!cur) return undefined;
    db.prepare('UPDATE run_configs SET name = ?, command = ?, port = ?, docker = ? WHERE id = ?').run(
      patch.name ?? cur.name,
      patch.command ?? cur.command,
      patch.port === undefined ? cur.port : patch.port,
      (patch.docker ?? cur.docker) ? 1 : 0,
      id,
    );
    return runConfigs.get(id);
  },
  setPrimary(projectId: string, id: string) {
    db.prepare('UPDATE run_configs SET is_primary = (id = ?) WHERE project_id = ?').run(id, projectId);
  },
  delete(id: string) {
    const cur = runConfigs.get(id);
    db.prepare('DELETE FROM run_configs WHERE id = ?').run(id);
    // the primary one was deleted — the first remaining one becomes primary
    if (cur?.primary) {
      const next = runConfigs.list(cur.projectId)[0];
      if (next) runConfigs.setPrimary(cur.projectId, next.id);
    }
  },
};
