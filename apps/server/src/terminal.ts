import { childEnv } from './config.ts';
import { execFileSync, spawn, type ChildProcess } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { homedir } from 'node:os';
import type { JobInfo } from '@hubness/shared';

/**
 * Terminal tasks: installing, logging in, updating harnesses.
 *
 * The command runs via `script` (util-linux): it gives the process a real
 * pty, so interactive CLIs (login via link with a code, picking a provider
 * with arrow keys) work just like in a terminal. The browser renders output in xterm.js
 * and sends keystrokes back.
 *
 * Who starts tasks: harness actions, clone, and also project Run and Shell
 * (commands of the hub owner; see runs.ts, environments.ts).
 */

const OUTPUT_LIMIT = 512 * 1024;
const KEEP_AFTER_EXIT_MS = 15 * 60_000;

/** Escaping for sh: 'a'\''b'. */
export const shq = (s: string) => (/^[\w@%+=:,./-]+$/.test(s) ? s : `'${s.replace(/'/g, `'\\''`)}'`);

interface Job {
  info: JobInfo;
  child: ChildProcess;
  output: string;
  listeners: Set<(msg: JobMsg) => void>;
  onExit?: (code: number | null) => void;
  onKill?: () => void;
  stopped?: boolean;
}

export type JobMsg = { type: 'output'; data: string } | { type: 'exit'; code: number | null };

const jobs = new Map<string, Job>();

export function startJob(opts: {
  title: string;
  command: string;
  /** What to show instead of the command (sandbox/container wrappers are long). */
  display?: string;
  env?: Record<string, string>;
  cwd?: string;
  cols?: number;
  rows?: number;
  onOutput?: (data: string) => void;
  onExit?: (code: number | null) => void;
  /** Extra stop step (a process in the container doesn't die with the docker exec client). */
  onKill?: () => void;
}): JobInfo {
  const cols = Math.max(40, Math.min(240, Math.round(opts.cols ?? 100)));
  const rows = Math.max(10, Math.min(80, Math.round(opts.rows ?? 30)));
  // set the pty size before running the command; -e — command exit code, -f — no buffering
  const inner = `stty cols ${cols} rows ${rows} 2>/dev/null; ${opts.command}`;
  // util-linux script: -q quiet, -f flush, -e child's exit code, -c command; BSD/macOS script
  // takes the command after the log file and passes the exit code through by itself
  const args = process.platform === 'darwin' ? ['-q', '-F', '/dev/null', '/bin/bash', '-c', inner] : ['-qfec', inner, '/dev/null'];
  const child = spawn('script', args, {
    cwd: opts.cwd ?? homedir(),
    // script runs the command via $SHELL — needs a POSIX shell, not the user's fish/zsh
    env: { ...childEnv(), ...opts.env, SHELL: '/bin/bash', TERM: 'xterm-256color', COLORTERM: 'truecolor' },
    stdio: ['pipe', 'pipe', 'pipe'],
    detached: true,
  });
  const info: JobInfo = {
    id: randomUUID(),
    title: opts.title,
    command: opts.display ?? opts.command,
    status: 'running',
    exitCode: null,
    startedAt: new Date().toISOString(),
  };
  const job: Job = { info, child, output: '', listeners: new Set(), onExit: opts.onExit, onKill: opts.onKill };
  jobs.set(info.id, job);

  // Primary Device Attributes query (ESC [ c / ESC [ 0 c): answered here, at once, the way
  // xterm.js would — shells like fish 4 wait up to 10 s for it at start, often before any
  // browser is attached. The browser terminal drops its own reply (see Terminal.tsx).
  let tail = '';
  const answerQueries = (data: string) => {
    const prev = tail;
    const text = prev + data;
    tail = text.slice(-3);
    // count only queries that end in the new chunk (one split across chunks is still seen once)
    for (const m of text.matchAll(/\x1b\[0?c/g)) if (m.index + m[0].length > prev.length) child.stdin?.write('\x1b[?1;2c');
  };
  const push = (data: string) => {
    answerQueries(data);
    job.output += data;
    if (job.output.length > OUTPUT_LIMIT) job.output = job.output.slice(-OUTPUT_LIMIT);
    opts.onOutput?.(data);
    for (const l of job.listeners) l({ type: 'output', data });
  };
  child.stdout!.on('data', (d: Buffer) => push(d.toString()));
  child.stderr!.on('data', (d: Buffer) => push(d.toString()));
  child.on('error', (err) => push(`\r\n[hub] failed to start: ${err.message}\r\n`));
  // typing into a process that just exited: EPIPE must not crash the hub
  child.stdin?.on('error', () => {});
  child.on('close', (exit) => {
    // a task stopped by the user has no exit code (script exits with 0 on SIGTERM)
    const code = job.stopped ? null : exit;
    info.status = 'exited';
    info.exitCode = code;
    // reset caches first (onExit), then notify the client — otherwise it re-reads stale data
    job.onExit?.(code);
    for (const l of job.listeners) l({ type: 'exit', code });
    setTimeout(() => jobs.delete(info.id), KEEP_AFTER_EXIT_MS).unref();
  });
  return info;
}

export const jobListeners = (id: string) => jobs.get(id)?.listeners.size ?? 0;

export function getJob(id: string) {
  const j = jobs.get(id);
  return j ? { info: j.info, output: j.output } : undefined;
}

export function subscribeJob(id: string, fn: (msg: JobMsg) => void): () => void {
  const j = jobs.get(id);
  if (!j) return () => {};
  j.listeners.add(fn);
  return () => j.listeners.delete(fn);
}

export function writeJob(id: string, data: string): boolean {
  const j = jobs.get(id);
  if (!j || j.info.status !== 'running' || !j.child.stdin?.writable) return false;
  j.child.stdin.write(data);
  return true;
}

/** Process children: /proc on Linux, pgrep elsewhere (macOS). */
function childPids(pid: number): number[] {
  try {
    const raw =
      process.platform === 'linux'
        ? readFileSync(`/proc/${pid}/task/${pid}/children`, 'utf8')
        : execFileSync('pgrep', ['-P', String(pid)], { encoding: 'utf8', timeout: 2000, stdio: ['ignore', 'pipe', 'ignore'], env: childEnv() });
    return raw.trim().split(/\s+/).filter(Boolean).map(Number);
  } catch {
    return [];
  }
}

/**
 * Stop the whole task. script runs the command in its own pty session
 * (its own process group), so a signal to script's group doesn't reach it —
 * we also signal its children's groups; after 3 s — SIGKILL.
 */
function killTree(j: Job) {
  const pid = j.child.pid;
  if (!pid) return;
  const groups = [pid, ...childPids(pid)];
  const signal = (sig: NodeJS.Signals) => {
    for (const g of groups) {
      try {
        process.kill(-g, sig);
      } catch {}
    }
  };
  // SIGHUP — like closing a terminal window: interactive shells ignore
  // SIGTERM but exit immediately on SIGHUP; then SIGTERM; after 3 s — SIGKILL
  signal('SIGHUP');
  signal('SIGTERM');
  setTimeout(() => signal('SIGKILL'), 3000).unref();
}

export function killJob(id: string): boolean {
  const j = jobs.get(id);
  if (!j || j.info.status !== 'running') return false;
  j.stopped = true;
  try {
    j.onKill?.();
  } catch {}
  killTree(j);
  return true;
}

/** On hub exit: stop all tasks (dev servers, shells) so they don't become orphans. */
export function killAllJobs() {
  for (const j of jobs.values()) if (j.info.status === 'running') killJob(j.info.id);
}

/** Non-interactive run (logout, switching): stdout+stderr and exit code. */
export function runOnce(cmd: string, args: string[], env?: Record<string, string>, timeoutMs = 60_000) {
  return new Promise<{ code: number | null; output: string }>((resolve) => {
    const p = spawn(cmd, args, { stdio: ['ignore', 'pipe', 'pipe'], env: { ...childEnv(), ...env } });
    let output = '';
    const timer = setTimeout(() => p.kill(), timeoutMs);
    p.stdout.on('data', (d: Buffer) => (output += d.toString()));
    p.stderr.on('data', (d: Buffer) => (output += d.toString()));
    p.on('error', (e) => (clearTimeout(timer), resolve({ code: null, output: e.message })));
    p.on('close', (code) => (clearTimeout(timer), resolve({ code, output: output.replace(/\x1b\[[0-9;]*m/g, '').trim() })));
  });
}
