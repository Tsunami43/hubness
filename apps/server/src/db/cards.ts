import { randomUUID } from 'node:crypto';
import type { Card, CardColumn } from '@hubness/shared';
import { db, now, type Row } from './connection.ts';

// ---------- cards ----------

const toCard = (r: Row): Card => ({
  id: r.id as string,
  projectId: r.project_id as string,
  title: r.title as string,
  description: r.description as string,
  column: r.col as CardColumn,
  position: Number(r.position),
  sessionId: (r.session_id as string) ?? null,
  createdAt: r.created_at as string,
  updatedAt: r.updated_at as string,
});

export const cards = {
  list(projectId: string): Card[] {
    return db
      .prepare('SELECT * FROM cards WHERE project_id = ? ORDER BY col, position')
      .all(projectId)
      .map(toCard);
  },
  get(id: string): Card | undefined {
    const r = db.prepare('SELECT * FROM cards WHERE id = ?').get(id);
    return r ? toCard(r) : undefined;
  },
  create(projectId: string, input: { title: string; description?: string; column?: CardColumn }): Card {
    const id = randomUUID();
    const t = now();
    const col = input.column ?? 'todo';
    const r = db
      .prepare('SELECT COALESCE(MAX(position), 0) AS m FROM cards WHERE project_id = ? AND col = ?')
      .get(projectId, col);
    db.prepare(
      `INSERT INTO cards (id, project_id, title, description, col, position, created_at, updated_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
    ).run(id, projectId, input.title, input.description ?? '', col, Number(r?.m ?? 0) + 1, t, t);
    return cards.get(id)!;
  },
  update(
    id: string,
    patch: Partial<Pick<Card, 'title' | 'description' | 'column' | 'position' | 'sessionId'>>,
  ): Card | undefined {
    const cur = cards.get(id);
    if (!cur) return undefined;
    let position = patch.position;
    if (patch.column && patch.column !== cur.column && position === undefined) {
      const r = db
        .prepare('SELECT COALESCE(MAX(position), 0) AS m FROM cards WHERE project_id = ? AND col = ?')
        .get(cur.projectId, patch.column);
      position = Number(r?.m ?? 0) + 1;
    }
    // fields not given (undefined) keep their current values
    const given = Object.fromEntries(Object.entries(patch).filter(([, v]) => v !== undefined));
    const next = { ...cur, ...given, position: position ?? cur.position };
    db.prepare(
      `UPDATE cards SET title = ?, description = ?, col = ?, position = ?, session_id = ?, updated_at = ? WHERE id = ?`,
    ).run(next.title, next.description, next.column, next.position, next.sessionId, now(), id);
    return cards.get(id);
  },
  delete(id: string) {
    db.prepare('DELETE FROM cards WHERE id = ?').run(id);
  },
};
