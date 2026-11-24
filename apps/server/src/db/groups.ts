import { randomUUID } from 'node:crypto';
import type { ProjectGroup } from '@hubness/shared';
import { db, now, tx, type Row } from './connection.ts';

// ---------- project groups ----------

const toGroup = (r: Row): ProjectGroup => ({
  id: r.id as string,
  name: r.name as string,
  description: (r.description as string) ?? '',
  instructions: (r.instructions as string) ?? '',
  folder: (r.folder as string) || null,
  projectIds: db
    .prepare('SELECT project_id FROM group_members WHERE group_id = ? ORDER BY position, rowid')
    .all(r.id as string)
    .map((x) => x.project_id as string),
  createdAt: r.created_at as string,
  updatedAt: r.updated_at as string,
});

export const groups = {
  list(): ProjectGroup[] {
    return db.prepare('SELECT * FROM project_groups ORDER BY name COLLATE NOCASE').all().map(toGroup);
  },
  get(id: string): ProjectGroup | undefined {
    const r = db.prepare('SELECT * FROM project_groups WHERE id = ?').get(id);
    return r ? toGroup(r) : undefined;
  },
  create(input: { name: string; description?: string; instructions?: string; folder?: string | null; projectIds: string[] }): ProjectGroup {
    const id = randomUUID();
    const t = now();
    tx(() => {
      db.prepare('INSERT INTO project_groups (id, name, description, instructions, folder, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)').run(
        id,
        input.name,
        input.description ?? '',
        input.instructions ?? '',
        input.folder ?? null,
        t,
        t,
      );
      groups.setMembersTx(id, input.projectIds);
    });
    return groups.get(id)!;
  },
  update(id: string, patch: { name?: string; description?: string; instructions?: string; folder?: string | null; projectIds?: string[] }): ProjectGroup | undefined {
    const cur = groups.get(id);
    if (!cur) return undefined;
    tx(() => {
      db.prepare('UPDATE project_groups SET name = ?, description = ?, instructions = ?, folder = ?, updated_at = ? WHERE id = ?').run(
        patch.name ?? cur.name,
        patch.description ?? cur.description,
        patch.instructions ?? cur.instructions,
        patch.folder === undefined ? cur.folder : patch.folder,
        now(),
        id,
      );
      if (patch.projectIds) groups.setMembersTx(id, patch.projectIds);
    });
    return groups.get(id);
  },
  /** Inside a transaction: replace the group's members (order as in the list). */
  setMembersTx(id: string, projectIds: string[]) {
    db.prepare('DELETE FROM group_members WHERE group_id = ?').run(id);
    projectIds.forEach((pid, i) =>
      db.prepare('INSERT INTO group_members (group_id, project_id, position) VALUES (?, ?, ?)').run(id, pid, i),
    );
  },
  touch(id: string) {
    db.prepare('UPDATE project_groups SET updated_at = ? WHERE id = ?').run(now(), id);
  },
  delete(id: string) {
    db.prepare('DELETE FROM project_groups WHERE id = ?').run(id);
  },
};
