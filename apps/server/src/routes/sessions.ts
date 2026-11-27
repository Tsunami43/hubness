import { readFileSync } from 'node:fs';
import { extname } from 'node:path';
import { randomUUID } from 'node:crypto';
import { Hono } from 'hono';
import { streamSSE } from 'hono/streaming';
import { z } from 'zod';
import type { AgentConfig, GlobalStreamMsg, Pane, SessionStreamMsg } from '@hubness/shared';
import { bus } from '../bus.ts';
import { events, groups, kv, projects, sessions } from '../db/index.ts';
import { groupWorkspace, projectWorkspace } from '../workspaces/index.ts';
import { convertToProject, inboxDirFor, suggestion } from '../inbox.ts';
import { groupOr404, groupProjects, projectOr404 } from '../catalog.ts';
import { HttpError } from '../errors.ts';
import { readFileSafe, safeJoin } from '../fsops.ts';
import * as git from '../git.ts';
import { attachmentPath, MAX_ATTACHMENT, resolveAttachments, saveAttachment } from '../attachments.ts';
import { isActive, respondApproval, sendMessage, sessionCwd, stopSession } from '../runtime.ts';
import { browserAction, closeSessionBrowser } from '../browser.ts';
import { previewPortFor } from '../runs.ts';
import { AgentConfigSchema, parseBody, pumpSSE, shortText, validateAgent } from './http.ts';

/** Sessions: CRUD, messages and attachments, approvals, live streams, Inbox → Project, the pane next to the chat. */
export const sessionRoutes = new Hono();

/** Types an attachment is served with: images and videos inline, the rest as a download. */
const MIME_FOR_SERVE: Record<string, string> = {
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.gif': 'image/gif',
  '.webp': 'image/webp',
  '.mp4': 'video/mp4',
  '.webm': 'video/webm',
  '.mov': 'video/quicktime',
};

const CreateSession = z.object({
  projectId: z.string().nullish(),
  groupId: z.string().nullish(),
  title: shortText(500).optional(),
  agent: AgentConfigSchema,
  prompt: z.string().max(1_000_000).optional(),
});
const PatchSession = z.object({ title: shortText(500).optional(), agent: AgentConfigSchema.partial().optional() });
const Message = z.object({ text: z.string().max(1_000_000).default(''), attachments: z.unknown().optional() });
const Approval = z.object({
  decision: z.enum(['allow', 'deny'], { error: 'decision: allow | deny' }),
  reason: z.string().max(10_000).optional(),
  answers: z.record(z.string(), z.string()).optional(),
});
const Convert = z.object({ name: shortText().default('') });
const PaneBody = z.object({
  pane: z
    .object({
      kind: z.enum(['url', 'file', 'diff'], { error: 'kind: url | file | diff' }),
      target: z.coerce.string(),
      title: z.string().optional(),
    })
    .nullable(),
});

const sessionOr404 = (id: string) => {
  const s = sessions.get(id);
  if (!s) throw new HttpError(404, 'Session not found');
  return s;
};

const announce = (id: string) => {
  const s = sessions.get(id)!;
  bus.emitSession(id, { type: 'session', session: s });
  bus.emitGlobal({ type: 'session', session: s });
  return s;
};

sessionRoutes.get('/sessions', (c) => {
  const projectId = c.req.query('projectId');
  if (projectId) return c.json(sessions.list({ projectId }));
  const groupId = c.req.query('groupId');
  if (groupId) return c.json(sessions.list({ groupId }));
  // archived=1: the archive (every scope); all=1: all sessions (history in the sidebar); otherwise only regular chats
  if (c.req.query('archived') === '1') return c.json(sessions.list({ archived: true }));
  return c.json(c.req.query('all') === '1' ? sessions.list({}) : sessions.list({ chats: true }));
});

/** Delete one session for good: stop its agent, drop it from the hub. */
function deleteSessionNow(id: string) {
  stopSession(id);
  // its page in the agents' browser goes with it
  void closeSessionBrowser(id);
  sessions.delete(id);
  bus.emitGlobal({ type: 'session-deleted', id });
}

/** Empty the archive (the client confirms first). */
sessionRoutes.delete('/sessions/archive', (c) => {
  const list = sessions.list({ archived: true });
  for (const s of list) deleteSessionNow(s.id);
  return c.json({ ok: true, deleted: list.length });
});

/** Move a session to the archive / back. A working agent is not archived. */
sessionRoutes.post('/sessions/:id/archive', (c) => {
  const id = sessionOr404(c.req.param('id')).id;
  if (isActive(id)) throw new HttpError(409, 'The agent is still working — wait for it or stop it');
  sessions.setArchived(id, true);
  void closeSessionBrowser(id);
  return c.json(announce(id));
});
sessionRoutes.post('/sessions/:id/restore', (c) => {
  const id = sessionOr404(c.req.param('id')).id;
  sessions.setArchived(id, false);
  return c.json(announce(id));
});

sessionRoutes.post('/sessions', async (c) => {
  const b = await parseBody(c, CreateSession);
  // a session has exactly one scope: project, group, or regular chat
  if (b.projectId && b.groupId) throw new HttpError(400, 'A session belongs to a project or a group, not both');
  let workspaceId: string | null = null;
  if (b.projectId) workspaceId = projectWorkspace(projectOr404(b.projectId)).id;
  if (b.groupId) {
    const g = groupOr404(b.groupId);
    workspaceId = groupWorkspace(g.id, groupProjects(g)).id;
  }
  const id = randomUUID();
  const scoped = Boolean(b.projectId || b.groupId);
  const s = sessions.create({
    id,
    projectId: b.projectId ?? null,
    groupId: b.groupId ?? null,
    workspaceId,
    title: b.title?.trim() || (scoped ? 'New session' : 'New chat'),
    agent: validateAgent(b.agent),
    // regular chat: its own folder in Inbox (~/hub/inbox/<date>-<topic>)
    cwd: scoped ? null : inboxDirFor(id),
  });
  bus.emitGlobal({ type: 'session', session: s });
  return c.json(b.prompt?.trim() ? sendMessage(s.id, b.prompt) : s, 201);
});

sessionRoutes.get('/sessions/:id', (c) => c.json(sessionOr404(c.req.param('id'))));

sessionRoutes.patch('/sessions/:id', async (c) => {
  const cur = sessionOr404(c.req.param('id'));
  const b = await parseBody(c, PatchSession);
  let agent: AgentConfig | undefined;
  if (b.agent) {
    // don't change the agent within a session: the conversation context lives with that specific agent;
    // the account stays: old sessions resume where their transcript lives
    agent = { ...validateAgent({ ...cur.agent, ...b.agent, kind: cur.agent.kind }), account: cur.agent.account };
  }
  sessions.update(cur.id, { title: b.title?.trim() || undefined, agent });
  return c.json(announce(cur.id));
});

sessionRoutes.delete('/sessions/:id', (c) => {
  deleteSessionNow(c.req.param('id'));
  return c.json({ ok: true });
});

sessionRoutes.get('/sessions/:id/events', (c) => c.json(events.list(c.req.param('id'))));

sessionRoutes.post('/sessions/:id/messages', async (c) => {
  const b = await parseBody(c, Message);
  const s = sessionOr404(c.req.param('id'));
  const files = resolveAttachments(sessionCwd(s), b.attachments);
  // writing to an archived session brings it back
  if (s.archivedAt && (b.text.trim() || files.length)) sessions.setArchived(s.id, false);
  return c.json(sendMessage(s.id, b.text, files));
});

/**
 * Attach a file to the next message: the raw bytes in the body, the name in X-Filename
 * (URI-encoded), the type in Content-Type. Saved into the session's working folder.
 */
sessionRoutes.post('/sessions/:id/attachments', async (c) => {
  const s = sessionOr404(c.req.param('id'));
  if (Number(c.req.header('content-length') ?? 0) > MAX_ATTACHMENT) throw new HttpError(413, `The file is larger than ${MAX_ATTACHMENT / 1024 / 1024} MB`);
  let name = 'file';
  try {
    name = decodeURIComponent(c.req.header('x-filename') ?? 'file');
  } catch {}
  const bytes = new Uint8Array(await c.req.arrayBuffer());
  return c.json(saveAttachment(sessionCwd(s), name, c.req.header('content-type') ?? '', bytes), 201);
});

/** An attachment's content (thumbnails and downloads in the feed). */
sessionRoutes.get('/sessions/:id/attachments/:file', (c) => {
  const s = sessionOr404(c.req.param('id'));
  const path = attachmentPath(sessionCwd(s), c.req.param('file'));
  const mime = MIME_FOR_SERVE[extname(path).toLowerCase()] ?? 'application/octet-stream';
  // never render attached HTML/SVG as a page of the hub's origin
  return c.body(readFileSync(path), 200, {
    'Content-Type': mime,
    'Content-Disposition': mime.startsWith('image/') || mime.startsWith('video/') ? 'inline' : 'attachment',
    'X-Content-Type-Options': 'nosniff',
    'Content-Security-Policy': "default-src 'none'; img-src 'self'; media-src 'self'; style-src 'unsafe-inline'",
  });
});

sessionRoutes.post('/sessions/:id/stop', (c) => c.json({ ok: stopSession(c.req.param('id')) }));

sessionRoutes.post('/sessions/:id/approvals/:requestId', async (c) => {
  const b = await parseBody(c, Approval);
  const ok = respondApproval(c.req.param('id'), c.req.param('requestId'), b);
  if (!ok) throw new HttpError(404, 'The request is no longer pending');
  return c.json({ ok: true });
});

// ---------- live streams ----------

sessionRoutes.get('/sessions/:id/stream', (c) => {
  const s = sessionOr404(c.req.param('id'));
  return streamSSE(c, (stream) =>
    pumpSSE<SessionStreamMsg>(stream, (push) => bus.onSession(s.id, push), {
      first: { type: 'snapshot', session: s, events: events.list(s.id), pane: kv.get<Pane | null>(`pane:${s.id}`)?.value ?? null },
      // the keep-alive carries the current session: a missed status change heals itself
      ping: () => ({ type: 'session', session: sessions.get(s.id) ?? s }),
    }),
  );
});

sessionRoutes.get('/stream', (c) => streamSSE(c, (stream) => pumpSSE<GlobalStreamMsg>(stream, (push) => bus.onGlobal(push))));

// ---------- Inbox → Project ----------

sessionRoutes.get('/sessions/:id/project-suggestion', (c) => c.json(suggestion(sessionOr404(c.req.param('id')))));

sessionRoutes.post('/sessions/:id/convert', async (c) => {
  const s = sessionOr404(c.req.param('id'));
  if (isActive(s.id)) throw new HttpError(409, 'The agent is still working — wait for it or stop it');
  const b = await parseBody(c, Convert);
  const project = await convertToProject(s, b.name);
  return c.json({ project, session: announce(s.id) });
});

// ---------- pane next to the chat (MCP open_pane) ----------

sessionRoutes.post('/sessions/:id/pane', async (c) => {
  const id = sessionOr404(c.req.param('id')).id;
  const b = await parseBody(c, PaneBody);
  const pane: Pane | null = b.pane ? { kind: b.pane.kind, target: b.pane.target.slice(0, 2000), title: b.pane.title?.slice(0, 200), at: new Date().toISOString() } : null;
  kv.set(`pane:${id}`, pane);
  bus.emitSession(id, { type: 'pane', pane });
  return c.json({ ok: true });
});

// ---------- the agent's headless browser (MCP preview_* tools) ----------

const BrowserBody = z.object({
  action: z.enum(['open', 'screenshot', 'click', 'type', 'press', 'eval', 'console', 'close']),
  url: z.string().max(4000).optional(),
  width: z.number().int().min(240).max(3840).optional(),
  height: z.number().int().min(240).max(4000).optional(),
  selector: z.string().max(2000).optional(),
  text: z.string().max(2000).optional(),
  value: z.string().max(100_000).optional(),
  key: z.string().max(40).optional(),
  expression: z.string().max(100_000).optional(),
  fullPage: z.boolean().optional(),
  screenshot: z.boolean().optional(),
});

sessionRoutes.post('/sessions/:id/browser', async (c) => {
  const s = sessionOr404(c.req.param('id'));
  const b = await parseBody(c, BrowserBody);
  const project = s.projectId ? projects.get(s.projectId) : null;
  return c.json(await browserAction(s.id, b, project ? (port) => previewPortFor(project, port) : null));
});

/** File for the pane: relative to the session folder (or absolute inside it / the project folders). */
function paneRoots(sessionId: string): string[] {
  const s = sessionOr404(sessionId);
  if (s.projectId) {
    const p = projects.get(s.projectId);
    return p ? [p.path] : [];
  }
  if (s.groupId) {
    const g = groups.get(s.groupId);
    return g ? [...groupProjects(g).map((p) => p.path), sessionCwd(s)] : [];
  }
  return [sessionCwd(s)];
}

function resolvePaneFile(sessionId: string, target: string): { root: string; rel: string } {
  for (const root of paneRoots(sessionId)) {
    const rel = target.startsWith('/') ? (target.startsWith(root + '/') ? target.slice(root.length + 1) : null) : target;
    if (rel !== null) return { root, rel };
  }
  throw new HttpError(400, 'File is outside the session folders');
}

sessionRoutes.get('/sessions/:id/file', (c) => {
  const { root, rel } = resolvePaneFile(c.req.param('id'), c.req.query('path') ?? '');
  return c.json(readFileSafe(root, rel));
});

sessionRoutes.get('/sessions/:id/diff', async (c) => {
  const { root, rel } = resolvePaneFile(c.req.param('id'), c.req.query('path') ?? '');
  safeJoin(root, rel);
  const st = await git.status(root);
  const f = st.files.find((x) => x.path === rel);
  const diff = await git.diff(root, { path: rel, untracked: f?.untracked });
  const staged = f && f.index !== ' ' && !f.untracked ? await git.diff(root, { path: rel, staged: true }) : '';
  return c.json({ diff: [staged, diff].filter(Boolean).join('\n') });
});
