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
import { sessionSetup, untestedSsh, type Owner, type SessionSsh } from './ssh.ts';

/**
 * A running agent process. `answered`: the agent has finished its turn (a `result`) but the
 * process lingers — Claude Code waits for its background tasks before it exits. The session
 * is idle for the user then; the agent may still come back on its own (a background task
 * finished), which makes it running again. `retired`: replaced by a newer turn or stopped
 * after its answer — whatever it still says is dropped.
 */
interface ActiveTurn {
  handle: RunHandle;
  answered: boolean;
  retired: boolean;
}

const active = new Map<string, ActiveTurn>();

/** Event kinds that mean the agent is at work again (new events, not updates of old ones). */
const WORK_KINDS = new Set<HubEvent['kind']>(['text', 'thinking', 'tool', 'approval']);

/** Default titles — replaced by the first message (Russian ones are for old records). */
const DEFAULT_TITLES = new Set(['New session', 'New chat', 'Новая сессия', 'Новый чат']);

export { HttpError };

/** The agent is at work in the session (a process that only lingers after its answer is not). */
export function isActive(sessionId: string) {
  const t = active.get(sessionId);
  return Boolean(t && !t.answered);
}

/**
 * Stop the session's process quietly: nothing it says reaches the feed. For one lingering after
 * its answer, and for a session being deleted (its events would have no session to belong to).
 */
export function retire(sessionId: string) {
  const t = active.get(sessionId);
  if (!t) return;
  t.retired = true;
  active.delete(sessionId);
  t.handle.stop();
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
  if (isActive(sessionId)) throw new HttpError(409, 'The agent is still working — wait for it or stop it');
  if (!text.trim() && !attachments.length) throw new HttpError(400, 'Empty message');
  // the previous turn's process still waits for its background tasks: the new turn replaces it
  retire(sessionId);

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

  // this turn's place among the running ones (the handle comes once the agent starts)
  const turn: ActiveTurn = { handle: null as unknown as RunHandle, answered: false, retired: false };
  const put = (uid: string, body: EventBody, opts?: { persist?: boolean }) => {
    if (turn.retired) return;
    const prev = cache.get(uid) ?? events.get(sessionId, uid);
    const ev = { ...body, uid, seq: prev?.seq ?? seq++, ts: prev?.ts ?? new Date().toISOString() } as HubEvent;
    cache.set(uid, ev);
    if (opts?.persist !== false) events.put(sessionId, ev);
    bus.emitSession(sessionId, { type: 'event', event: ev });
    if (body.kind === 'result') {
      // the agent has answered: the turn is over for the user, even if the process lingers;
      // a turn that ends with a result is idle even when the answer is an error (API Error 529)
      turn.answered = true;
      finishTurn(sessionId, false);
    } else if (turn.answered && !prev && WORK_KINDS.has(body.kind)) {
      // it came back on its own (a background task finished)
      turn.answered = false;
      setSessionStatus(sessionId, 'running');
    }
  };

  // SSH connections of the project (a group: its own and its repositories'), for this turn
  const owners: Owner[] = project ? [{ project: project.id }] : group ? [{ group: group.id }, ...groupProjects(group).map((p) => ({ project: p.id }))] : [];
  // a broken setup (an unreadable key, no runtime folder) costs the turn its SSH, not the turn
  let ssh: SessionSsh | null = null;
  let sshProblem: string | null = null;
  let sshUntested: string[] = [];
  try {
    sshUntested = untestedSsh(owners);
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
    setStatus: (st) => {
      if (turn.retired) return;
      if (st === 'running' || st === 'waiting') turn.answered = false;
      setSessionStatus(sessionId, st);
    },
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
  if (sshUntested.length)
    put(`ssh-untested:${Date.now()}`, {
      kind: 'notice',
      level: 'info',
      text: `SSH: ${sshUntested.join(', ')} ${sshUntested.length > 1 ? 'are' : 'is'} not tested yet — press Test in System → SSH to let the agent use ${sshUntested.length > 1 ? 'them' : 'it'}.`,
    });
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
  turn.handle = handle;
  active.set(sessionId, turn);

  handle.done.finally(() => {
    closeLog();
    // the process is gone: its ssh-agent and config go away
    ssh?.dispose();
    // a retired process leaves the session to the turn that replaced it
    if (turn.retired || active.get(sessionId) !== turn) return;
    active.delete(sessionId);
    const all = events.list(sessionId);
    const last = all[all.length - 1];
    finishTurn(sessionId, last?.kind === 'notice' && last.level === 'error');
  });

  return sessions.get(sessionId)!;
}

/** The turn is over (answered, or its process is gone): idle or error, a kanban card to review. */
function finishTurn(sessionId: string, failed: boolean) {
  setSessionStatus(sessionId, failed ? 'error' : 'idle');
  // The session was started from a kanban card — when it finishes, the card goes to review
  const s = sessions.get(sessionId);
  if (s?.cardId && !failed) {
    const card = cards.get(s.cardId);
    if (card?.column === 'doing') cards.update(card.id, { column: 'review' });
  }
}

/** Stop the session's agent. One that has already answered is stopped quietly. */
export function stopSession(sessionId: string): boolean {
  const t = active.get(sessionId);
  if (!t) return false;
  if (t.answered) retire(sessionId);
  else t.handle.stop();
  return true;
}

export function respondApproval(sessionId: string, requestId: string, d: ApprovalDecision): boolean {
  const t = active.get(sessionId);
  if (!t) return false;
  return t.handle.respond(requestId, d);
}

/** Hub exit: the feed shows "interrupted by restart", not "stopped" (nothing for an answered turn). */
export function stopAll() {
  for (const t of active.values()) {
    if (t.answered) t.retired = true;
    t.handle.stop('shutdown');
  }
}
