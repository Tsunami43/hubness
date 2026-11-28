import { spawn, execFile } from 'node:child_process';
import { createInterface } from 'node:readline';
import type { ApprovalDecision, AskQuestion } from '@hubness/shared';
import { childEnv } from '../../config.ts';
import { recordClaudeLimits } from './harness.ts';
import { CLAUDE_BIN, claudeProfile, profileEnv } from './accounts.ts';
import { stopNotice, type AgentAdapter, type RunContext, type RunHandle, type StopReason } from '../types.ts';
import { isModelImage } from '../../attachments.ts';
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { runtimeBase } from '../../ssh.ts';

/**
 * Claude Code CLI adapter.
 * Protocol: `claude -p` with stream-json for input and output; permissions go through
 * control_request/can_use_tool on stdout and control_response on stdin
 * (`--permission-prompt-tool stdio`). Resume — `--resume <session_id>`.
 */

const OUTPUT_LIMIT = 20_000;

type Json = Record<string, any>;

function buildArgs(ctx: RunContext, mcpFile: string | null): string[] {
  const a = ctx.session.agent;
  const args = [
    '-p',
    '--verbose',
    '--output-format=stream-json',
    '--input-format=stream-json',
    '--include-partial-messages',
    '--permission-prompt-tool',
    'stdio',
  ];
  if (a.model) args.push('--model', a.model);
  switch (a.permissionMode ?? 'ask') {
    case 'bypass':
      args.push('--dangerously-skip-permissions');
      break;
    case 'acceptEdits':
      args.push('--permission-mode', 'acceptEdits');
      break;
    case 'plan':
      args.push('--permission-mode', 'plan');
      break;
    default:
      args.push('--permission-mode', 'default');
  }
  for (const d of ctx.extraDirs) args.push('--add-dir', d);
  if (ctx.systemPrompt) args.push('--append-system-prompt', ctx.systemPrompt);
  if (mcpFile) {
    args.push('--mcp-config', mcpFile);
    // hub tools are safe (they only show a pane), so don't ask for permission
    args.push('--allowedTools', ...ctx.mcpServers.flatMap((m) => [`mcp__${m.name}__open_pane`, `mcp__${m.name}__close_pane`]));
  }
  if (ctx.session.agentSessionId) args.push('--resume', ctx.session.agentSessionId);
  return args;
}

function stringifyToolOutput(content: unknown): string {
  let s: string;
  if (typeof content === 'string') s = content;
  else if (Array.isArray(content))
    s = content
      .map((c: Json) => (c?.type === 'text' ? c.text : c?.type === 'image' ? '[image]' : JSON.stringify(c)))
      .join('\n');
  else s = JSON.stringify(content ?? '');
  return s.length > OUTPUT_LIMIT ? s.slice(0, OUTPUT_LIMIT) + '\n… (truncated)' : s;
}

export const claudeAdapter: AgentAdapter = {
  start(ctx: RunContext): RunHandle {
    // the MCP config carries the hub's tokens in its env: a private file for this process, not
    // an argument (any local user can read a command line)
    const mcpDir = ctx.mcpServers.length ? join(runtimeBase(), `mcp-${randomUUID().slice(0, 12)}`) : null;
    let mcpFile: string | null = null;
    if (mcpDir) {
      mkdirSync(mcpDir, { mode: 0o700 });
      mcpFile = join(mcpDir, 'mcp.json');
      const mcpServers = Object.fromEntries(ctx.mcpServers.map((m) => [m.name, { command: m.command, args: m.args, env: m.env }]));
      writeFileSync(mcpFile, JSON.stringify({ mcpServers }), { mode: 0o600, flag: 'wx' });
    }
    let [cmd, args] = ctx.wrap ? ctx.wrap(CLAUDE_BIN, buildArgs(ctx, mcpFile)) : [CLAUDE_BIN, buildArgs(ctx, mcpFile)];
    // the sandbox hides the runtime folder: show it this one, read-only
    if (ctx.wrap && mcpDir) {
      const at = args.indexOf('--');
      args = [...args.slice(0, at), '--ro-bind', mcpDir, mcpDir, ...args.slice(at)];
    }
    const child = spawn(cmd, args, {
      cwd: ctx.cwd,
      env: { ...childEnv(), ...ctx.env, ...profileEnv(claudeProfile(ctx.session.agent.account)) },
      stdio: ['pipe', 'pipe', 'pipe'],
      detached: true, // own process group: stop() kills the whole tree
    });

    // a CLI that dies right away (bad binary, crash on start) closes stdin: the write fails
    // with EPIPE, and an unhandled stream error would take the whole hub down — the exit
    // handler reports the failed turn instead
    child.stdin.on('error', () => {});
    const send = (obj: Json) => {
      if (!child.stdin.writable) return;
      child.stdin.write(JSON.stringify(obj) + '\n');
    };

    let model: string | undefined;
    let apiKeySource: string | undefined; // 'none' means subscription OAuth login
    let limit: { window: string; utilization: number } | undefined;
    let gotResult = false;
    let stopped = false;
    let stopReason: StopReason = 'user';
    const stderr: string[] = [];
    const streamedMsgs = new Set<string>();
    const deniedTools = new Set<string>();
    // control_requests awaiting a response: request_id → request data
    const pending = new Map<string, Json>();
    // streaming: block index → event uid
    let curMsgId = '';
    const blockUid = new Map<number, { uid: string; kind: 'text' | 'thinking'; text: string }>();

    const finishPending = (status: 'expired') => {
      for (const id of pending.keys()) ctx.patch(`ap:${id}`, { status } as any);
      pending.clear();
    };

    const onStreamEvent = (e: Json) => {
      switch (e.type) {
        case 'message_start':
          curMsgId = e.message?.id ?? `m${Date.now()}`;
          blockUid.clear();
          break;
        case 'content_block_start': {
          const t = e.content_block?.type;
          if (t === 'text' || t === 'thinking') {
            const uid = `${t === 'text' ? 't' : 'th'}:${curMsgId}:${e.index}`;
            blockUid.set(e.index, { uid, kind: t, text: '' });
            streamedMsgs.add(curMsgId);
            if (t === 'text') ctx.put(uid, { kind: 'text', text: '', streaming: true }, { persist: false });
          }
          break;
        }
        case 'content_block_delta': {
          const b = blockUid.get(e.index);
          if (!b) break;
          if (e.delta?.type === 'text_delta') {
            b.text += e.delta.text ?? '';
            ctx.patch(b.uid, { text: b.text } as any, { persist: false });
          } else if (e.delta?.type === 'thinking_delta') {
            b.text += e.delta.thinking ?? '';
          }
          break;
        }
        case 'content_block_stop': {
          const b = blockUid.get(e.index);
          if (!b) break;
          if (b.kind === 'text') ctx.put(b.uid, { kind: 'text', text: b.text, streaming: false });
          else if (b.text.trim()) ctx.put(b.uid, { kind: 'thinking', text: b.text });
          blockUid.delete(e.index);
          break;
        }
      }
    };

    const onAssistant = (j: Json) => {
      const msg = j.message ?? {};
      const blocks: Json[] = Array.isArray(msg.content) ? msg.content : [];
      blocks.forEach((b, i) => {
        if (b.type === 'tool_use') {
          const uid = `tool:${b.id}`;
          if (!ctx.get(uid))
            ctx.put(uid, { kind: 'tool', toolId: b.id, name: b.name, input: b.input, status: 'running' });
        } else if (b.type === 'text' && !streamedMsgs.has(msg.id) && b.text) {
          ctx.put(`t:${msg.id}:a${i}`, { kind: 'text', text: b.text });
        } else if (b.type === 'thinking' && !streamedMsgs.has(msg.id) && b.thinking?.trim()) {
          ctx.put(`th:${msg.id}:a${i}`, { kind: 'thinking', text: b.thinking });
        }
      });
    };

    const onUser = (j: Json) => {
      const content = j.message?.content;
      if (!Array.isArray(content)) return;
      for (const c of content) {
        if (c.type !== 'tool_result') continue;
        const uid = `tool:${c.tool_use_id}`;
        if (!ctx.get(uid)) continue;
        const status = c.is_error ? (deniedTools.has(c.tool_use_id) ? 'denied' : 'error') : 'ok';
        ctx.patch(uid, { status, output: stringifyToolOutput(c.content) } as any);
      }
    };

    const onControlRequest = (j: Json) => {
      const req = j.request ?? {};
      if (req.subtype !== 'can_use_tool') {
        send({
          type: 'control_response',
          response: { subtype: 'error', request_id: j.request_id, error: `unsupported: ${req.subtype}` },
        });
        return;
      }
      pending.set(j.request_id, req);
      const questions: AskQuestion[] | undefined =
        req.tool_name === 'AskUserQuestion' && Array.isArray(req.input?.questions) ? req.input.questions : undefined;
      ctx.put(`ap:${j.request_id}`, {
        kind: 'approval',
        requestId: j.request_id,
        toolName: req.tool_name,
        input: req.input,
        status: 'pending',
        questions,
      });
      ctx.setStatus('waiting');
    };

    const rl = createInterface({ input: child.stdout });
    rl.on('line', (line) => {
      if (!line.trim()) return;
      let j: Json;
      try {
        j = JSON.parse(line);
      } catch {
        ctx.raw(line);
        return;
      }
      if (j.type !== 'stream_event') ctx.raw(line);
      // don't show subagent (Task) events, only the top level
      if (j.parent_tool_use_id) return;
      switch (j.type) {
        case 'system':
          if (j.subtype === 'init') {
            if (j.session_id) ctx.setAgentSessionId(j.session_id);
            model = j.model;
            apiKeySource = j.apiKeySource;
          }
          break;
        case 'rate_limit_event': {
          recordClaudeLimits(j.rate_limit_info, claudeProfile(ctx.session.agent.account).id);
          // subscription limits: take the most utilized window
          const windows = Object.entries((j.rate_limit_info?.unifiedWindows ?? {}) as Record<string, Json>);
          const top = windows.sort((a, b) => (b[1].utilization ?? 0) - (a[1].utilization ?? 0))[0];
          if (top && typeof top[1].utilization === 'number') limit = { window: top[0], utilization: top[1].utilization };
          break;
        }
        case 'stream_event':
          onStreamEvent(j.event ?? {});
          break;
        case 'assistant':
          if (j.message?.model) model = j.message.model;
          onAssistant(j);
          break;
        case 'user':
          onUser(j);
          break;
        case 'control_request':
          onControlRequest(j);
          break;
        case 'result':
          gotResult = true;
          if (j.session_id) ctx.setAgentSessionId(j.session_id);
          if (j.is_error && typeof j.result === 'string')
            ctx.put(`err:${j.uuid ?? Date.now()}`, { kind: 'notice', level: 'error', text: j.result });
          ctx.put(`res:${j.uuid ?? Date.now()}`, {
            kind: 'result',
            ok: !j.is_error,
            durationMs: j.duration_ms,
            billing: apiKeySource === 'none' ? 'subscription' : 'api',
            costUsd: j.total_cost_usd,
            limit,
            model,
          });
          child.stdin.end();
          break;
      }
    });

    child.stderr.on('data', (d: Buffer) => {
      const s = d.toString();
      ctx.raw(`[stderr] ${s.trimEnd()}`);
      stderr.push(s);
      if (stderr.length > 50) stderr.shift();
    });

    const done = new Promise<void>((resolve) => {
      child.on('error', (err) => {
        ctx.put(`err:spawn:${Date.now()}`, {
          kind: 'notice',
          level: 'error',
          text: `Failed to start Claude Code (${CLAUDE_BIN}): ${err.message}`,
        });
        resolve();
      });
      child.on('close', (code) => {
        finishPending('expired');
        if (stopped) {
          ctx.put(`stop:${Date.now()}`, { kind: 'notice', level: 'info', text: stopNotice(stopReason) });
        } else if (!gotResult) {
          const tail = stderr.join('').trim().split('\n').slice(-15).join('\n');
          ctx.put(`err:exit:${Date.now()}`, {
            kind: 'notice',
            level: 'error',
            text: `Claude Code exited with code ${code}${tail ? `:\n${tail}` : ''}`,
          });
        }
        resolve();
      });
    }).finally(() => {
      if (mcpDir) rmSync(mcpDir, { recursive: true, force: true });
    });

    // First user message
    // images go to the model as content blocks (paths are in the text for everything else)
    const images = ctx.attachments.filter(isModelImage).flatMap((a) => {
      try {
        return [{ type: 'image', source: { type: 'base64', media_type: a.mime, data: readFileSync(a.path).toString('base64') } }];
      } catch {
        return [];
      }
    });
    send({ type: 'user', message: { role: 'user', content: images.length ? [{ type: 'text', text: ctx.prompt }, ...images] : ctx.prompt } });

    return {
      done,
      stop(reason: StopReason = 'user') {
        stopped = true;
        stopReason = reason;
        try {
          if (child.pid) process.kill(-child.pid, 'SIGTERM');
        } catch {}
        setTimeout(() => {
          try {
            if (child.pid && child.exitCode === null) process.kill(-child.pid, 'SIGKILL');
          } catch {}
        }, 3000).unref();
      },
      respond(requestId: string, d: ApprovalDecision): boolean {
        const req = pending.get(requestId);
        if (!req) return false;
        pending.delete(requestId);
        let response: Json;
        if (d.decision === 'allow') {
          const updatedInput =
            req.tool_name === 'AskUserQuestion' && d.answers ? { ...req.input, answers: d.answers } : req.input;
          response = { behavior: 'allow', updatedInput };
          // Plan approval: switch the CLI session out of plan mode, as the CLI itself suggests
          if (req.tool_name === 'ExitPlanMode' && Array.isArray(req.permission_suggestions))
            response.updatedPermissions = req.permission_suggestions;
        } else {
          if (req.tool_use_id) deniedTools.add(req.tool_use_id);
          response = { behavior: 'deny', message: d.reason?.trim() || 'The user denied this action' };
        }
        send({ type: 'control_response', response: { subtype: 'success', request_id: requestId, response } });
        ctx.patch(`ap:${requestId}`, {
          status: d.decision === 'allow' ? 'allowed' : 'denied',
          answers: d.answers,
          reason: d.reason,
        } as any);
        if (pending.size === 0) ctx.setStatus('running');
        return true;
      },
    };
  },
};

export function claudeVersion(): Promise<string | null> {
  return new Promise((resolve) => {
    execFile(CLAUDE_BIN, ['--version'], { timeout: 10_000 }, (err, out) =>
      resolve(err ? null : out.trim()),
    );
  });
}
