import { appendFileSync, createWriteStream, mkdirSync, type WriteStream } from 'node:fs';
import { join } from 'node:path';
import type { ApprovalDecision, Attachment, HubEvent, Session, SessionStatus } from '@hubness/shared';
import { bus } from './bus.ts';
import { HttpError } from './errors.ts';
import { config } from './config.ts';
import { cards, events, groups, projects, sessions } from './db/index.ts';
import { groupAgentContext, groupProjects, sessionInstructions } from './catalog.ts';
import { projectContext, sessionSystemPrompt } from './context.ts';
import { attachmentsNote } from './attachments.ts';
import { groupWorkspace } from './workspaces/index.ts';
import { findAgent } from './agents/registry.ts';
import type { EventBody, McpServerSpec, RunContext, RunHandle } from './agents/types.ts';
import { fileURLToPath } from 'node:url';
import { agentWrap, runtimeOf } from './environments.ts';
import { secretEnv } from './secrets.ts';
import { sessionSetup, type Owner, type SessionSsh } from './ssh.ts';

const active = new Map<string, RunHandle>();

/** Default titles — replaced by the first message (Russian ones are for old records). */
const DEFAULT_TITLES = new Set(['New session', 'New chat', 'Новая сессия', 'Новый чат']);

export { HttpError };

export function isActive(sessionId: string) {
  return active.has(sessionId);
}

function emitSession(s: Session) {
  bus.emitSession(s.id, { type: 'session', session: s });
  bus.emitGlobal({ type: 'session', session: s });
}

export function setSessionStatus(id: string, status: SessionStatus) {
  const s = sessions.update(id, { status });
  if (s) emitSession(s);
}

export function sessionCwd(s: Session): string {
  if (s.projectId) {
    const p = projects.get(s.projectId);
    if (!p) throw new HttpError(404, 'Session project not found');
    return p.path;
  }
  if (s.groupId) {
    const g = groups.get(s.groupId);
    if (!g) throw new HttpError(404, 'Session group not found');
    return groupWorkspace(g.id, groupProjects(g)).path;
  }
  if (s.cwd) {
    mkdirSync(s.cwd, { recursive: true });
    return s.cwd;
  }
  // old chats from before Inbox
  const dir = join(config.chatsDir, s.id);
  mkdirSync(dir, { recursive: true });
  return dir;
}

const MCP_SCRIPT = fileURLToPath(new URL('./mcp/hub-mcp.ts', import.meta.url));

function hubMcp(sessionId: string): McpServerSpec {
  return {
    name: 'hub',
    command: process.execPath,
    args: ['--disable-warning=ExperimentalWarning', MCP_SCRIPT],
    env: { HUB_URL: config.selfUrl, HUB_SESSION_ID: sessionId },
  };
}

export function sendMessage(sessionId: string, text: string, attachments: Attachment[] = []): Session {
  let session = sessions.get(sessionId);
  if (!session) throw new HttpError(404, 'Session not found');
  if (active.has(sessionId)) throw new HttpError(409, 'The agent is still working — wait for it or stop it');
  if (!text.trim() && !attachments.length) throw new HttpError(400, 'Empty message');

  if (!session.title || DEFAULT_TITLES.has(session.title)) {
    const t = (text.trim() || attachments.map((a) => a.name).join(', ')).replace(/\s+/g, ' ');
    session = sessions.update(sessionId, { title: t.length > 60 ? t.slice(0, 57) + '…' : t })!;
  }
  if (session.projectId) projects.touch(session.projectId);
  if (session.groupId) groups.touch(session.groupId);

  const cwd = sessionCwd(session);
  const project = session.projectId ? projects.get(session.projectId) : undefined;
  const group = session.groupId ? groups.get(session.groupId) : undefined;
  // after a chat is moved into a project, the agent gets a conversation summary (once)
  const handoff = sessions.getHandoff(sessionId);
  if (handoff) sessions.setHandoff(sessionId, null);
  let seq = events.nextSeq(sessionId);
  const cache = new Map<string, HubEvent>();
  // raw agent output: an append stream for the whole turn (a sync write per line would block the event loop)
  const logFile = join(config.logsDir, `${sessionId}.jsonl`);
  let log: WriteStream | null = null;
  let turnOver = false;
  const closeLog = () => {
    turnOver = true;
    log?.end();
    log = null;
  };

  const put = (uid: string, body: EventBody, opts?: { persist?: boolean }) => {
    const prev = cache.get(uid) ?? events.get(sessionId, uid);
    const ev = { ...body, uid, seq: prev?.seq ?? seq++, ts: prev?.ts ?? new Date().toISOString() } as HubEvent;
    cache.set(uid, ev);
    if (opts?.persist !== false) events.put(sessionId, ev);
    bus.emitSession(sessionId, { type: 'event', event: ev });
  };

  // SSH connections of the project (a group: its own and its repositories'), for this turn
  const owners: Owner[] = project ? [{ project: project.id }] : group ? [{ group: group.id }, ...groupProjects(group).map((p) => ({ project: p.id }))] : [];
  // a broken setup (an unreadable key, no runtime folder) costs the turn its SSH, not the turn
  let ssh: SessionSsh | null = null;
  let sshProblem: string | null = null;
  try {
    ssh = sessionSetup(owners, sessionId);
    if (ssh?.failed.length) sshProblem = `SSH: the key of ${ssh.failed.join(', ')} could not be loaded — not available in this turn.`;
  } catch (e) {
    sshProblem = `SSH connections are not available in this turn: ${(e as Error).message}`;
  }
  const scopeContext = project ? projectContext(project) : group ? groupAgentContext(group) : null;
  const baseWrap = agentWrap(project ? { kind: 'project', project } : group ? { kind: 'group', group, projects: groupProjects(group), cwd } : null);
  const ctx: RunContext = {
    session,
    cwd,
    // the agent is told where attached files are; images also go to the model directly (adapters)
    prompt: (handoff ? `${handoff}\n${text}` : text) + attachmentsNote(attachments),
    attachments,
    // a group session sees exactly the member repositories (real paths, not symlinks)
    extraDirs: group ? groupProjects(group).map((p) => p.path) : [],
    systemPrompt: sessionSystemPrompt(sessionInstructions(), ssh ? [scopeContext, ssh.context].filter(Boolean).join('\n\n') : scopeContext),
    mcpServers: [hubMcp(sessionId)],
    env: { ...(project && runtimeOf(project).shareSecretsWithAgents ? secretEnv(project.id) : {}), ...(ssh?.env ?? {}) },
    // the sandbox hides the hub's folders: show it the session's ssh folder, read-only
    wrap:
      baseWrap && ssh
        ? (cmd, args) => {
            const [bin, full] = baseWrap(cmd, args);
            const at = full.indexOf('--');
            return [bin, [...full.slice(0, at), '--ro-bind', ssh.mount, ssh.mount, ...full.slice(at)]];
          }
        : baseWrap,
    put,
    patch(uid, patch, opts) {
      const prev = cache.get(uid) ?? events.get(sessionId, uid);
      if (!prev) return;
      const { uid: _u, seq: _s, ts: _t, ...body } = prev;
      put(uid, { ...body, ...patch } as EventBody, opts);
    },
    get: (uid) => cache.get(uid) ?? events.get(sessionId, uid),
    setStatus: (st) => setSessionStatus(sessionId, st),
    setAgentSessionId(id) {
      if (session!.agentSessionId === id) return;
      session = sessions.update(sessionId, { agentSessionId: id })!;
    },
    history: () => events.list(sessionId),
    raw(line) {
      if (turnOver) {
        // a straggler after the turn ended (rare): append directly, don't reopen a stream nobody closes
        try {
          appendFileSync(logFile, line + '\n');
        } catch {}
        return;
      }
      if (!log) {
        log = createWriteStream(logFile, { flags: 'a' });
        // a failed log write must never break the turn
        log.on('error', () => {});
      }
      log.write(line + '\n');
    },
  };

  put(`u:${Date.now()}`, { kind: 'user', text, ...(attachments.length ? { attachments } : {}) });
  ctx.raw(JSON.stringify({ hub: 'user', ts: new Date().toISOString(), text, attachments: attachments.map((a) => a.path) }));
  if (sshProblem) put(`ssh:${Date.now()}`, { kind: 'notice', level: 'error', text: sshProblem });
  setSessionStatus(sessionId, 'running');

  let handle: RunHandle;
  try {
    const adapter = findAgent(session.agent.kind)?.adapter;
    if (!adapter) throw new Error(`Agent “${session.agent.kind}” is not supported anymore — start a new session`);
    handle = adapter.start(ctx);
  } catch (err) {
    put(`err:${Date.now()}`, { kind: 'notice', level: 'error', text: (err as Error).message });
    setSessionStatus(sessionId, 'error');
    closeLog();
    ssh?.dispose();
    return sessions.get(sessionId)!;
  }
  active.set(sessionId, handle);

  handle.done.finally(() => {
    active.delete(sessionId);
    closeLog();
    // the turn is over: its ssh-agent and config go away
    ssh?.dispose();
    const all = events.list(sessionId);
    const last = all[all.length - 1];
    const failed = last?.kind === 'notice' && last.level === 'error';
    setSessionStatus(sessionId, failed ? 'error' : 'idle');
    // The session was started from a kanban card — when it finishes, the card goes to review
    const s = sessions.get(sessionId);
    if (s?.cardId && !failed) {
      const card = cards.get(s.cardId);
      if (card?.column === 'doing') cards.update(card.id, { column: 'review' });
    }
  });

  return sessions.get(sessionId)!;
}

export function stopSession(sessionId: string): boolean {
  const h = active.get(sessionId);
  if (!h) return false;
  h.stop();
  return true;
}

export function respondApproval(sessionId: string, requestId: string, d: ApprovalDecision): boolean {
  const h = active.get(sessionId);
  if (!h) return false;
  return h.respond(requestId, d);
}

/** Hub exit: the feed shows "interrupted by restart", not "stopped". */
export function stopAll() {
  for (const h of active.values()) h.stop('shutdown');
}
