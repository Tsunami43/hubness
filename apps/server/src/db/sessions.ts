import { randomUUID } from 'node:crypto';
import type { AgentConfig, HubEvent, Session, SessionStatus } from '@hubness/shared';
import { INTERRUPTED_NOTICE } from '../agents/types.ts';
import { db, now, type Row } from './connection.ts';

// ---------- sessions ----------

const toSession = (r: Row): Session => ({
  id: r.id as string,
  projectId: (r.project_id as string) ?? null,
  groupId: (r.group_id as string) ?? null,
  workspaceId: (r.workspace_id as string) ?? null,
  cardId: (r.card_id as string) ?? null,
  title: r.title as string,
  agent: JSON.parse(r.agent as string) as AgentConfig,
  agentSessionId: (r.agent_session_id as string) ?? null,
  cwd: (r.cwd as string) ?? null,
  status: r.status as SessionStatus,
  archivedAt: (r.archived_at as string) ?? null,
  createdAt: r.created_at as string,
  updatedAt: r.updated_at as string,
});

export const sessions = {
  /**
   * Sessions newest first. Archived ones are left out unless `archived` is set: true — only
   * the archive, 'all' — both (checks that must see every session, e.g. a running agent).
   */
  list(filter: { projectId?: string; groupId?: string; chats?: boolean; archived?: boolean | 'all' }): Session[] {
    const arch = filter.archived === 'all' ? '' : filter.archived ? ' AND archived_at IS NOT NULL' : ' AND archived_at IS NULL';
    const order = filter.archived === true ? 'archived_at DESC' : 'updated_at DESC';
    const q = (where: string, ...args: string[]) =>
      db.prepare(`SELECT * FROM sessions WHERE ${where}${arch} ORDER BY ${order}`).all(...args).map(toSession);
    if (filter.groupId) return q('group_id = ?', filter.groupId);
    if (filter.projectId) return q('project_id = ?', filter.projectId);
    if (filter.chats) return q('project_id IS NULL AND group_id IS NULL');
    return q('1 = 1');
  },
  /** Move to the archive (archived = true) or back. Does not bump updated_at. */
  setArchived(id: string, archived: boolean): Session | undefined {
    db.prepare('UPDATE sessions SET archived_at = ? WHERE id = ?').run(archived ? now() : null, id);
    return sessions.get(id);
  },
  get(id: string): Session | undefined {
    const r = db.prepare('SELECT * FROM sessions WHERE id = ?').get(id);
    return r ? toSession(r) : undefined;
  },
  create(input: {
    projectId: string | null;
    groupId?: string | null;
    workspaceId?: string | null;
    cardId?: string | null;
    title: string;
    agent: AgentConfig;
    cwd?: string | null;
    id?: string;
  }): Session {
    if (input.projectId && input.groupId) throw new Error('A session belongs to a project or a group, not both');
    const id = input.id ?? randomUUID();
    const t = now();
    db.prepare(
      `INSERT INTO sessions (id, project_id, group_id, workspace_id, card_id, title, agent, cwd, status, created_at, updated_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'idle', ?, ?)`,
    ).run(
      id,
      input.projectId,
      input.groupId ?? null,
      input.workspaceId ?? null,
      input.cardId ?? null,
      input.title,
      JSON.stringify(input.agent),
      input.cwd ?? null,
      t,
      t,
    );
    return sessions.get(id)!;
  },
  /** Move a chat into a project (Inbox → Project). */
  moveToProject(id: string, projectId: string) {
    db.prepare('UPDATE sessions SET project_id = ?, cwd = NULL, updated_at = ? WHERE id = ?').run(projectId, now(), id);
  },
  getHandoff(id: string): string | null {
    const r = db.prepare('SELECT handoff FROM sessions WHERE id = ?').get(id);
    return (r?.handoff as string) ?? null;
  },
  setHandoff(id: string, text: string | null) {
    db.prepare('UPDATE sessions SET handoff = ? WHERE id = ?').run(text, id);
  },
  update(
    id: string,
    patch: Partial<Pick<Session, 'title' | 'agent' | 'agentSessionId' | 'status' | 'cardId'>>,
  ): Session | undefined {
    const sets: string[] = [];
    const vals: (string | null)[] = [];
    if (patch.title !== undefined) (sets.push('title = ?'), vals.push(patch.title));
    if (patch.agent !== undefined) (sets.push('agent = ?'), vals.push(JSON.stringify(patch.agent)));
    if (patch.agentSessionId !== undefined) (sets.push('agent_session_id = ?'), vals.push(patch.agentSessionId));
    if (patch.status !== undefined) (sets.push('status = ?'), vals.push(patch.status));
    if (patch.cardId !== undefined) (sets.push('card_id = ?'), vals.push(patch.cardId));
    sets.push('updated_at = ?');
    vals.push(now());
    db.prepare(`UPDATE sessions SET ${sets.join(', ')} WHERE id = ?`).run(...vals, id);
    return sessions.get(id);
  },
  delete(id: string) {
    db.prepare('DELETE FROM sessions WHERE id = ?').run(id);
  },
};

// ---------- events ----------

export const events = {
  list(sessionId: string): HubEvent[] {
    return db
      .prepare('SELECT data FROM events WHERE session_id = ? ORDER BY seq')
      .all(sessionId)
      .map((r) => JSON.parse(r.data as string) as HubEvent);
  },
  get(sessionId: string, uid: string): HubEvent | undefined {
    const r = db.prepare('SELECT data FROM events WHERE session_id = ? AND uid = ?').get(sessionId, uid);
    return r ? (JSON.parse(r.data as string) as HubEvent) : undefined;
  },
  nextSeq(sessionId: string): number {
    const r = db.prepare('SELECT COALESCE(MAX(seq), 0) AS m FROM events WHERE session_id = ?').get(sessionId);
    return Number(r?.m ?? 0) + 1;
  },
  put(sessionId: string, ev: HubEvent) {
    db.prepare(
      `INSERT INTO events (session_id, uid, seq, data) VALUES (?, ?, ?, ?)
       ON CONFLICT(session_id, uid) DO UPDATE SET data = excluded.data`,
    ).run(sessionId, ev.uid, ev.seq, JSON.stringify(ev));
  },
};

/**
 * After a server restart no agent is running. Sessions cut off mid-run go back to idle and get
 * a notice in the feed (the agent's own exit notice is often lost: the hub exits before the child does).
 */
export function recoverInterruptedSessions() {
  const interrupted = db
    .prepare(`SELECT id FROM sessions WHERE status IN ('running', 'waiting')`)
    .all()
    .map((r) => r.id as string);
  db.exec(`UPDATE sessions SET status = 'idle' WHERE status IN ('running', 'waiting')`);
  for (const id of interrupted) {
    const all = events.list(id);
    const last = all[all.length - 1];
    if (last?.kind === 'notice' && last.text === INTERRUPTED_NOTICE) continue;
    events.put(id, { kind: 'notice', level: 'info', text: INTERRUPTED_NOTICE, uid: `restart:${Date.now()}`, seq: events.nextSeq(id), ts: now() } as HubEvent);
  }
}
