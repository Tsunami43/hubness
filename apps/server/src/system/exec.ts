import { spawn } from 'node:child_process';
import { statSync } from 'node:fs';
import { delimiter, join } from 'node:path';

/** Running official CLIs for System pages (C locale, no colors, with a timeout). */

export interface CmdResult {
  code: number;
  out: string;
  err: string;
}

export function sh(cmd: string, args: string[], timeoutMs = 20_000): Promise<CmdResult> {
  return new Promise((resolve) => {
    const p = spawn(cmd, args, { stdio: ['ignore', 'pipe', 'pipe'], env: { ...process.env, LC_ALL: 'C', SYSTEMD_COLORS: '0' } });
    let out = '';
    let err = '';
    const timer = setTimeout(() => p.kill(), timeoutMs);
    p.stdout.on('data', (d: Buffer) => (out += d.toString()));
    p.stderr.on('data', (d: Buffer) => (err += d.toString()));
    p.on('error', (e) => (clearTimeout(timer), resolve({ code: 127, out, err: e.message })));
    p.on('close', (code) => (clearTimeout(timer), resolve({ code: code ?? 1, out, err })));
  });
}

export function onPath(bin: string): boolean {
  for (const dir of (process.env.PATH ?? '').split(delimiter)) {
    if (!dir) continue;
    try {
      if (statSync(join(dir, bin)).isFile()) return true;
    } catch {}
  }
  return false;
}
