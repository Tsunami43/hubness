import { childEnv } from '../config.ts';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import type { AgentKind, ApprovalDecision, HubEvent, PermissionMode } from '@hubness/shared';
import { kv } from '../db/index.ts';
import { stopNotice, type AgentAdapter, type McpServerSpec, type RunContext, type RunHandle, type StopReason } from './types.ts';
import { isModelImage } from '../attachments.ts';
import { readFileSync } from 'node:fs';

/**
 * Shared ACP (Agent Client Protocol) client: JSON-RPC 2.0 over stdio.
 * Turn: initialize → session/new | session/load (resume) → model and mode →
 * session/prompt. Streaming via session/update notifications, permissions via
 * session/request_permission requests.
 *
 * Everything that differs between agents (launch command, how to pass MCP, how to enable
 * no-prompt mode) is described in AcpSpec; the client itself is agent-agnostic.
 */

type Json = Record<string, any>;
const OUTPUT_LIMIT = 20_000;

export interface AcpSpec {
  kind: AgentKind;
  /** Name used in error messages. */
  label: string;
  /** Command that launches the agent's ACP server. */
  command(ctx: RunContext, mode: PermissionMode): { cmd: string; args: string[]; env?: Record<string, string> };
  /** Pass hub MCP servers as the ACP mcpServers parameter (otherwise the agent gets them itself via command/env). */
  mcpViaParam: boolean;
  /** _meta for session/new and session/load (e.g. Grok's yolo mode). */
  sessionMeta?(mode: PermissionMode): Json | undefined;
  /** Agent mode id (session/set_mode) for our permission mode, if the agent declares it. */
  modeIds?: Partial<Record<PermissionMode, string>>;
  /** The agent applies permission modes itself (via config); no client-side policy needed. */
  nativePermissions?: boolean;
  /**
   * Service session update (session_info_update) → a line for the feed:
   * e.g. the agent retrying a request after a provider error. null: don't show.
   */
  describeInfo?(update: Json): { retry: string; error: string } | null;
  /** Extra setup after creating/loading a session. */
  afterSession?(call: (method: string, params: Json) => Promise<any>, sessionId: string, mode: PermissionMode): Promise<void>;
}

/** Models the agent declared in the last session (for selection in the UI). */
export const modelsKey = (kind: AgentKind) => `models:${kind}`;

function rememberModels(kind: AgentKind, r: Json) {
  const opt = (r?.configOptions as Json[] | undefined)?.find((o) => o.id === 'model');
  const fromConfig = (opt?.options as Json[] | undefined)?.flatMap((o) =>
    Array.isArray(o.options) ? o.options.map((x: Json) => x.value) : [o.value],
  );
  const fromModels = (r?.models?.availableModels as Json[] | undefined)?.map((m) => m.modelId);
  const list = (fromConfig?.length ? fromConfig : fromModels)?.filter((x): x is string => typeof x === 'string');
  if (list?.length) kv.set(modelsKey(kind), list);
}

function currentModel(r: Json): string | undefined {
  return r?.configOptions?.find((o: Json) => o.id === 'model')?.currentValue ?? r?.models?.currentModelId;
}

/**
 * Agent tools → canonical names/fields the UI understands.
 * title can be either the tool name (write) or the command itself (bash),
 * so look at the input fields first.
 */
function detectTool(title: string, raw: Json): string {
  const t = (title || '').toLowerCase();
  if (['write', 'edit', 'read', 'bash', 'grep', 'glob', 'webfetch', 'todowrite', 'task', 'list'].includes(t)) return t;
  if (typeof raw.command === 'string' || Array.isArray(raw.command)) return 'bash';
  if (raw.oldString !== undefined || raw.old_string !== undefined) return 'edit';
  if (raw.content !== undefined && (raw.filePath ?? raw.path ?? raw.file_path)) return 'write';
  if (Array.isArray(raw.todos)) return 'todowrite';
  if (raw.url) return 'webfetch';
  if (raw.pattern) return raw.include || raw.path ? 'grep' : 'glob';
  if (raw.filePath ?? raw.file_path) return 'read';
  return t;
}

export function normalizeTool(title: string, raw: Json): { name: string; input: Json } {
  // Grok calls MCP tools through a meta-tool: search and call by name
  if (raw.variant === 'UseTool' && typeof raw.tool_name === 'string') return normalizeTool(raw.tool_name, raw.tool_input ?? {});
  if (raw.variant === 'SearchTool') return { name: 'ToolSearch', input: { query: raw.query } };
  if (raw.variant === 'ListDir') return { name: 'LS', input: { path: raw.target_directory } };
  // hub MCP tools: map to the same names as in Claude Code
  if (/open_pane/.test(title) || (raw.kind && raw.target && ['url', 'file', 'diff'].includes(raw.kind)))
    return { name: 'mcp__hub__open_pane', input: raw };
  if (/close_pane/.test(title)) return { name: 'mcp__hub__close_pane', input: raw };
  const t = detectTool(title, raw);
  const path = raw.filePath ?? raw.path ?? raw.file_path;
  switch (t) {
    case 'write':
      return { name: 'Write', input: { file_path: path, content: raw.content } };
    case 'edit':
      return {
        name: 'Edit',
        input: { file_path: path, old_string: raw.oldString ?? raw.old_string, new_string: raw.newString ?? raw.new_string },
      };
    case 'read':
      return { name: 'Read', input: { file_path: path } };
    case 'bash':
      return {
        name: 'Bash',
        input: { command: Array.isArray(raw.command) ? raw.command.join(' ') : raw.command, description: raw.description },
      };
    case 'grep':
      return { name: 'Grep', input: { pattern: raw.pattern, path: raw.path } };
    case 'glob':
      return { name: 'Glob', input: { pattern: raw.pattern } };
    case 'webfetch':
      return { name: 'WebFetch', input: { url: raw.url } };
    case 'todowrite':
      return { name: 'TodoWrite', input: { todos: raw.todos } };
    case 'task':
      return { name: 'Task', input: { description: raw.description ?? raw.prompt } };
    default:
      return { name: title && title.length <= 40 ? title : 'Tool', input: raw };
  }
}

/** ACP content of type diff → an edit in our format (if there is no tool input). */
function diffTool(content: unknown): { name: string; input: Json } | null {
  if (!Array.isArray(content)) return null;
  const d = content.find((c: Json) => c?.type === 'diff');
  if (!d) return null;
  return d.oldText
    ? { name: 'Edit', input: { file_path: d.path, old_string: d.oldText, new_string: d.newText } }
    : { name: 'Write', input: { file_path: d.path, content: d.newText } };
}

function contentText(content: unknown): string {
  if (!Array.isArray(content)) return '';
  const s = content
    .map((c: Json) => {
      if (c?.type === 'content' && c.content?.type === 'text') return c.content.text;
      if (c?.type === 'diff') return `diff ${c.path}`;
      return '';
    })
    .filter(Boolean)
    .join('\n');
  return s.length > OUTPUT_LIMIT ? s.slice(0, OUTPUT_LIMIT) + '\n… (truncated)' : s;
}

const WRITE_KINDS = ['edit', 'delete', 'move'];

/**
 * Client-side policy for agents that have no modes of their own:
 * allow/deny: answer immediately; ask: ask the user.
 */
function autoDecision(mode: PermissionMode, toolKind: string | undefined): 'allow' | 'deny' | 'ask' {
  if (mode === 'bypass') return 'allow';
  const writes = WRITE_KINDS.includes(toolKind ?? '');
  if (mode === 'acceptEdits' && writes) return 'allow';
  if (mode === 'plan' && writes) return 'deny';
  return 'ask';
}

/** Session history recap for an agent that can't load its own sessions. */
function transcript(history: HubEvent[]): string {
  const lines: string[] = [];
  for (const e of history) {
    if (e.kind === 'user') lines.push(`User: ${e.text}`);
    else if (e.kind === 'text' && e.text.trim()) lines.push(`Assistant: ${e.text}`);
  }
  const s = lines.join('\n\n');
  return s.length > 30_000 ? '…\n' + s.slice(-30_000) : s;
}

const mcpParam = (servers: McpServerSpec[]) =>
  servers.map((m) => ({ name: m.name, command: m.command, args: m.args, env: Object.entries(m.env).map(([name, value]) => ({ name, value })) }));

export function createAcpAdapter(spec: AcpSpec): AgentAdapter {
  return {
    start(ctx: RunContext): RunHandle {
      const mode = ctx.session.agent.permissionMode ?? 'ask';
      const base = spec.command(ctx, mode);
      const [cmd, args] = ctx.wrap ? ctx.wrap(base.cmd, base.args) : [base.cmd, base.args];
      const child = spawn(cmd, args, {
        cwd: ctx.cwd,
        env: { ...childEnv(), ...ctx.env, ...base.env },
        stdio: ['pipe', 'pipe', 'pipe'],
        detached: true,
      });

      let nextId = 0;
      const calls = new Map<number, { resolve: (v: any) => void; reject: (e: any) => void }>();
      // EPIPE from an agent that exited right away must not crash the hub (the exit handler reports it)
      child.stdin.on('error', () => {});
      const send = (obj: Json) => child.stdin.writable && child.stdin.write(JSON.stringify(obj) + '\n');
      const call = (method: string, params: Json) =>
        new Promise<any>((resolve, reject) => {
          const id = ++nextId;
          calls.set(id, { resolve, reject });
          send({ jsonrpc: '2.0', id, method, params });
        });

      let sessionId: string | null = ctx.session.agentSessionId;
      let isNew = !sessionId;
      let replaying = false; // session/load replays history; don't show it
      let stopped = false;
      let stopReason: StopReason = 'user';
      let finished = false;
      // last provider error the agent reported during retries (its text is clearer than the final one)
      let lastProviderError: string | null = null;
      let model: string | undefined = ctx.session.agent.model;
      const stderr: string[] = [];
      const started = Date.now();
      const runTag = Date.now().toString(36);
      let textBlock: { uid: string; text: string } | null = null;
      let thoughtBlock: { uid: string; text: string } | null = null;
      let blockN = 0;
      const pending = new Map<number, Json>(); // jsonrpc id of the permission request → params
      const toolInput = new Map<string, { name: string; input: Json }>();

      const flushText = () => {
        if (textBlock) ctx.put(textBlock.uid, { kind: 'text', text: textBlock.text, streaming: false });
        textBlock = null;
      };
      const flushThought = () => {
        if (thoughtBlock?.text.trim()) ctx.put(thoughtBlock.uid, { kind: 'thinking', text: thoughtBlock.text });
        thoughtBlock = null;
      };

      const onUpdate = (u: Json) => {
        if (replaying) return;
        switch (u.sessionUpdate) {
          case 'agent_message_chunk': {
            flushThought();
            const piece = u.content?.type === 'text' ? u.content.text : '';
            if (!textBlock) textBlock = { uid: `t:${runTag}:${++blockN}`, text: '' };
            textBlock.text += piece ?? '';
            ctx.put(textBlock.uid, { kind: 'text', text: textBlock.text, streaming: true }, { persist: false });
            break;
          }
          case 'agent_thought_chunk': {
            flushText();
            if (!thoughtBlock) thoughtBlock = { uid: `th:${runTag}:${++blockN}`, text: '' };
            thoughtBlock.text += u.content?.text ?? '';
            break;
          }
          case 'tool_call':
          case 'tool_call_update': {
            flushText();
            flushThought();
            const uid = `tool:${u.toolCallId}`;
            const prev = ctx.get(uid);
            const hasInput = u.rawInput && Object.keys(u.rawInput).length > 0;
            let norm = hasInput
              ? normalizeTool(u.title ?? (prev?.kind === 'tool' ? prev.name.toLowerCase() : ''), u.rawInput)
              : toolInput.get(u.toolCallId) ?? diffTool(u.content) ?? normalizeTool(u.title ?? '', {});
            // input has no path/edit text but there is a diff: take the edit from it
            const diff = diffTool(u.content);
            if (diff && (norm.name === 'Edit' || norm.name === 'Write') && !norm.input.new_string && norm.input.content === undefined)
              norm = diff;
            const known = hasInput || toolInput.has(u.toolCallId) || Boolean(diff);
            if (known) toolInput.set(u.toolCallId, norm);
            const status =
              u.status === 'completed' ? 'ok' : u.status === 'failed' ? 'error' : prev?.kind === 'tool' ? prev.status : 'running';
            const output = contentText(u.content) || (prev?.kind === 'tool' ? prev.output : undefined);
            const keepPrev = prev?.kind === 'tool' && !known;
            ctx.put(uid, {
              kind: 'tool',
              toolId: u.toolCallId,
              name: keepPrev ? prev.name : norm.name,
              input: keepPrev ? prev.input : norm.input,
              status: prev?.kind === 'tool' && prev.status === 'denied' ? 'denied' : status,
              output,
            });
            break;
          }
          case 'session_info_update': {
            const info = spec.describeInfo?.(u);
            if (info) {
              lastProviderError = info.error;
              ctx.put(`retry:${runTag}`, { kind: 'notice', level: 'info', text: info.retry });
            }
            break;
          }
          case 'plan': {
            const todos = (u.entries ?? []).map((e: Json) => ({ content: e.content, status: e.status }));
            ctx.put(`plan:${runTag}`, { kind: 'tool', toolId: `plan:${runTag}`, name: 'TodoWrite', input: { todos }, status: 'ok' });
            break;
          }
        }
      };

      const answer = (id: number, params: Json, decision: 'allow' | 'deny') => {
        const options: Json[] = params.options ?? [];
        const want = decision === 'allow' ? ['allow_once', 'allow_always'] : ['reject_once', 'reject_always'];
        const opt = want.map((k) => options.find((o) => o.kind === k)).find(Boolean);
        send({
          jsonrpc: '2.0',
          id,
          result: opt ? { outcome: { outcome: 'selected', optionId: opt.optionId } } : { outcome: { outcome: 'cancelled' } },
        });
        if (decision === 'deny' && params.toolCall?.toolCallId)
          ctx.patch(`tool:${params.toolCall.toolCallId}`, { status: 'denied' } as any);
      };

      const onPermission = (id: number, params: Json) => {
        const tc = params.toolCall ?? {};
        if (replaying) return answer(id, params, 'deny');
        const norm =
          tc.rawInput && Object.keys(tc.rawInput).length
            ? normalizeTool(toolInput.get(tc.toolCallId)?.name.toLowerCase() ?? tc.title ?? tc.kind ?? '', tc.rawInput)
            : toolInput.get(tc.toolCallId) ?? diffTool(tc.content) ?? { name: tc.title ?? 'tool', input: {} };
        // hub tools (pane) don't change anything: like Claude Code, no prompt
        if (norm.name.startsWith('mcp__hub__')) return answer(id, params, 'allow');
        if (!spec.nativePermissions) {
          const auto = autoDecision(mode, tc.kind);
          if (auto !== 'ask') return answer(id, params, auto);
        }
        flushText();
        pending.set(id, params);
        ctx.put(`ap:${id}:${runTag}`, {
          kind: 'approval',
          requestId: `${id}:${runTag}`,
          toolName: norm.name,
          input: norm.input,
          status: 'pending',
        });
        ctx.setStatus('waiting');
      };

      createInterface({ input: child.stdout }).on('line', (line) => {
        if (!line.trim()) return;
        let m: Json;
        try {
          m = JSON.parse(line);
        } catch {
          ctx.raw(line);
          return;
        }
        const isChunk = m.method === 'session/update' && /_chunk$/.test(m.params?.update?.sessionUpdate ?? '');
        if (!isChunk && !replaying) ctx.raw(line);
        if (m.id !== undefined && !m.method) {
          const h = calls.get(m.id);
          calls.delete(m.id);
          if (m.error) h?.reject(m.error);
          else h?.resolve(m.result);
          return;
        }
        if (m.method === 'session/update') return onUpdate(m.params?.update ?? {});
        if (m.method === 'session/request_permission') return onPermission(m.id, m.params ?? {});
        if (m.id !== undefined) send({ jsonrpc: '2.0', id: m.id, error: { code: -32601, message: 'Method not found' } });
      });

      child.stderr.on('data', (d: Buffer) => {
        const s = d.toString();
        ctx.raw(`[stderr] ${s.trimEnd()}`);
        stderr.push(s);
        if (stderr.length > 50) stderr.shift();
      });

      const kill = () => {
        try {
          if (child.pid) process.kill(-child.pid, 'SIGTERM');
        } catch {}
      };

      const mcpServers = spec.mcpViaParam ? mcpParam(ctx.mcpServers) : [];
      const meta = spec.sessionMeta?.(mode);
      // extra folders (group repositories) only for agents that declare them
      // (sessionCapabilities.additionalDirectories); the rest see them via
      // symlinks in cwd (group workspace)
      let extraDirsSupported = false;
      const sessionParams = () => ({
        cwd: ctx.cwd,
        mcpServers,
        ...(extraDirsSupported && ctx.extraDirs.length ? { additionalDirectories: ctx.extraDirs } : {}),
        ...(meta ? { _meta: meta } : {}),
      });

      const run = async () => {
        const init = await call('initialize', {
          protocolVersion: 1,
          clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false },
        });
        extraDirsSupported = Boolean(init?.agentCapabilities?.sessionCapabilities?.additionalDirectories);
        // images go to the model only if the agent says it takes them; otherwise just the path
        const imagesSupported = Boolean(init?.agentCapabilities?.promptCapabilities?.image);
        if (ctx.attachments.length) ctx.raw(JSON.stringify({ hub: 'promptCapabilities', value: init?.agentCapabilities?.promptCapabilities ?? null }));
        let preface = '';
        let r: Json = {};
        if (sessionId && init?.agentCapabilities?.loadSession) {
          replaying = true;
          try {
            r = (await call('session/load', { sessionId, ...sessionParams() })) ?? {};
          } finally {
            replaying = false;
          }
        } else {
          if (sessionId) {
            // the agent can't load sessions: continue in a new one with a recap
            // everything before the message being sent now (the last user event)
            const hist = ctx.history();
            const cut = hist.map((e) => e.kind).lastIndexOf('user');
            const prev = transcript(cut >= 0 ? hist.slice(0, cut) : hist);
            if (prev) preface = `Earlier conversation in this session (for context):\n\n${prev}\n\n---\n\n`;
            isNew = true;
          }
          r = await call('session/new', sessionParams());
          sessionId = r.sessionId as string;
          ctx.setAgentSessionId(sessionId);
        }
        const sid = sessionId!;
        rememberModels(spec.kind, r);
        if (!model) model = currentModel(r);
        if (ctx.session.agent.model) {
          const value = ctx.session.agent.model;
          await call('session/set_config_option', { sessionId: sid, configId: 'model', value }).catch(() =>
            call('session/set_model', { sessionId: sid, modelId: value }),
          );
        }
        const modeId = spec.modeIds?.[mode];
        const available: Json[] = r.modes?.availableModes ?? [];
        if (modeId && available.some((m) => m.id === modeId))
          await call('session/set_mode', { sessionId: sid, modeId }).catch(() => {});
        await spec.afterSession?.(call, sid, mode);
        // system context (manifest) goes into the first message of a new session
        const head = isNew && ctx.systemPrompt ? `${ctx.systemPrompt}\n\n---\n\n` : '';
        const images = imagesSupported
          ? ctx.attachments.filter(isModelImage).flatMap((f) => {
              try {
                return [{ type: 'image', mimeType: f.mime, data: readFileSync(f.path).toString('base64') }];
              } catch {
                return [];
              }
            })
          : [];
        const res = await call('session/prompt', { sessionId: sid, prompt: [{ type: 'text', text: head + preface + ctx.prompt }, ...images] });
        flushThought();
        flushText();
        finished = true;
        ctx.put(`res:${runTag}`, {
          kind: 'result',
          ok: res?.stopReason === 'end_turn' || res?.stopReason === 'max_tokens',
          stopped: res?.stopReason === 'cancelled' || undefined,
          durationMs: Date.now() - started,
          model,
        });
      };

      const done = new Promise<void>((resolve) => {
        let settled = false;
        const end = () => {
          if (settled) return;
          settled = true;
          for (const id of pending.keys()) ctx.patch(`ap:${id}:${runTag}`, { status: 'expired' } as any);
          pending.clear();
          kill();
          resolve();
        };
        child.on('error', (err) => {
          ctx.put(`err:spawn:${runTag}`, {
            kind: 'notice',
            level: 'error',
            text: `Failed to start ${spec.label} (${cmd}): ${err.message}`,
          });
          end();
        });
        child.on('close', (code) => {
          // settled: the hub stopped the process (after an error or a response); this is not an agent crash
          if (!finished && !stopped && !settled) {
            const tail = stderr.join('').trim().split('\n').slice(-15).join('\n');
            ctx.put(`err:exit:${runTag}`, {
              kind: 'notice',
              level: 'error',
              text: `${spec.label} exited with code ${code}${tail ? `:\n${tail}` : ''}`,
            });
          }
          end();
        });
        run()
          .catch((err: Json) => {
            flushText();
            if (stopped) return;
            const msg = err?.message ?? String(err);
            ctx.put(`err:${runTag}`, { kind: 'notice', level: 'error', text: lastProviderError ? `${msg}\n${lastProviderError}` : msg });
          })
          .finally(() => {
            if (stopped) ctx.put(`stop:${runTag}`, { kind: 'notice', level: 'info', text: stopNotice(stopReason) });
            end();
          });
      });

      return {
        done,
        stop(reason: StopReason = 'user') {
          stopped = true;
          stopReason = reason;
          if (sessionId) send({ jsonrpc: '2.0', method: 'session/cancel', params: { sessionId } });
          setTimeout(kill, 1500).unref();
        },
        respond(requestId: string, d: ApprovalDecision): boolean {
          const id = Number(requestId.split(':')[0]);
          const params = pending.get(id);
          if (!params) return false;
          pending.delete(id);
          answer(id, params, d.decision);
          ctx.patch(`ap:${requestId}`, { status: d.decision === 'allow' ? 'allowed' : 'denied', reason: d.reason } as any);
          if (pending.size === 0) ctx.setStatus('running');
          return true;
        },
      };
    },
  };
}
