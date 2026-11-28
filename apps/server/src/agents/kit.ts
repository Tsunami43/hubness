import { readFileSync, realpathSync } from 'node:fs';
import { spawn } from 'node:child_process';
import type { HarnessAction, HarnessVersion } from '@hubness/shared';
import { childEnv } from '../config.ts';
import { shq } from '../terminal.ts';

/**
 * Helpers for agent modules reading their harness: running the CLI, versions and
 * the "latest version" network cache, actions. Agent-agnostic.
 */

export type Json = Record<string, any>;

/** Run a CLI with stdin closed (otherwise some CLIs wait for input and print nothing). null — failed or timed out. */
export function runCli(cmd: string, args: string[], timeoutMs = 20_000, env?: Record<string, string>): Promise<string | null> {
  return new Promise((resolve) => {
    const p = spawn(cmd, args, { stdio: ['ignore', 'pipe', 'ignore'], env: { ...childEnv(), ...env } });
    let out = '';
    const timer = setTimeout(() => p.kill(), timeoutMs);
    p.stdout.on('data', (d: Buffer) => (out += d.toString()));
    p.on('error', () => (clearTimeout(timer), resolve(null)));
    p.on('close', (code) => (clearTimeout(timer), resolve(code === 0 ? out : null)));
  });
}

export function readJson(path: string): Json | null {
  try {
    return JSON.parse(readFileSync(path, 'utf8'));
  } catch {
    return null;
  }
}

export const parseVersion = (s: string | null | undefined) => s?.match(/(\d+\.\d+\.\d+)/)?.[1] ?? null;

export function cmpVersion(a: string, b: string): number {
  const pa = a.split('.').map(Number);
  const pb = b.split('.').map(Number);
  for (let i = 0; i < 3; i++) if ((pa[i] ?? 0) !== (pb[i] ?? 0)) return (pa[i] ?? 0) - (pb[i] ?? 0);
  return 0;
}

export function versionInfo(installed: string | null, latest: string | null, source: string, updateCommand: string | null): HarnessVersion {
  if (!installed) return { installed, latest, status: 'not-installed', source, updateCommand };
  if (!latest) return { installed, latest, status: 'unknown', source, updateCommand };
  return {
    installed,
    latest,
    status: cmpVersion(installed, latest) < 0 ? 'update-available' : 'up-to-date',
    source,
    updateCommand,
  };
}

// latest versions from the network, per agent kind, cached for an hour
const latestCache = new Map<string, { at: number; v: string | null }>();

export async function cachedLatest(kind: string, key: string, fetcher: () => Promise<string | null>): Promise<string | null> {
  const k = `${kind}\0${key}`;
  const c = latestCache.get(k);
  if (c && Date.now() - c.at < 3600_000) return c.v;
  const v = await fetcher().catch(() => null);
  latestCache.set(k, { at: Date.now(), v });
  return v;
}

/** Forget the cached latest versions of one agent (its refresh, or after an update). */
export function clearLatest(kind: string) {
  for (const k of [...latestCache.keys()]) if (k.startsWith(`${kind}\0`)) latestCache.delete(k);
}

export async function npmLatest(pkg: string): Promise<string | null> {
  const res = await fetch(`https://registry.npmjs.org/${pkg}/latest`, { signal: AbortSignal.timeout(10_000) });
  if (!res.ok) return null;
  return parseVersion(((await res.json()) as Json).version);
}

/** Full path of the binary (via PATH), with symlinks resolved. */
export async function resolveBin(bin: string): Promise<string | null> {
  const out = await runCli('sh', ['-c', `command -v ${shq(bin)}`], 5_000);
  const p = out?.trim();
  if (!p) return null;
  try {
    return realpathSync(p);
  } catch {
    return p;
  }
}

export const action = (id: HarnessAction['id'], command: string, opts: Partial<Omit<HarnessAction, 'id' | 'command'>> = {}): HarnessAction => ({
  id,
  command,
  runnable: opts.runnable ?? true,
  interactive: opts.interactive ?? true,
});

export const notFound = (bin: string) => `Not installed (“${bin}” not found) — see Harness`;
