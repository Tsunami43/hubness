import { spawn, spawnSync } from 'node:child_process';
import { readFileSync, realpathSync, statSync } from 'node:fs';
import { join } from 'node:path';
import type { SearchFile, SearchOptions, SearchResult } from '@hubness/shared';
import { childEnv } from './config.ts';
import { HttpError } from './errors.ts';
import { listFiles } from './fsops.ts';

/**
 * Text search across project files (the Search view, like VS Code's). ripgrep when it is
 * installed (fast, respects .gitignore, skips binaries); otherwise a plain walk over the
 * project files. Results are capped so a broad query can't flood the browser.
 */

const MAX_MATCHES = 2000;
const MAX_PER_FILE = 200;
const MAX_LINE = 400;
const TIMEOUT_MS = 15_000;

let rgOk: boolean | null = null;
const hasRg = () => (rgOk ??= spawnSync('rg', ['--version'], { timeout: 3000, env: childEnv() }).status === 0);

const globs = (s: string | undefined) =>
  (s ?? '')
    .split(',')
    .map((x) => x.trim())
    .filter(Boolean);

/** UTF-8 byte offset inside `text` → UTF-16 index (what the browser works with). */
const utf16 = (text: string, bytes: number) => Buffer.from(text, 'utf8').subarray(0, bytes).toString('utf8').length;

export async function searchFiles(root: string, opts: SearchOptions): Promise<SearchResult> {
  const q = opts.q ?? '';
  if (!q.trim()) return { files: [], total: 0, truncated: false };
  if (q.length > 500) throw new HttpError(400, 'Query is too long');
  if (opts.regex) {
    try {
      new RegExp(q);
    } catch (e) {
      throw new HttpError(400, `Invalid regular expression: ${(e as Error).message}`);
    }
  }
  const cwd = realpathSync(root);
  return hasRg() ? searchRg(cwd, q, opts) : searchNode(cwd, q, opts);
}

function searchRg(cwd: string, q: string, opts: SearchOptions): Promise<SearchResult> {
  const args = ['--json', '--hidden', '--glob', '!.git', '--max-columns', String(MAX_LINE), '--max-columns-preview', '--max-filesize', '2M', '--max-count', String(MAX_PER_FILE)];
  args.push(opts.caseSensitive ? '--case-sensitive' : '--ignore-case');
  if (opts.word) args.push('--word-regexp');
  if (!opts.regex) args.push('--fixed-strings');
  for (const g of globs(opts.include)) args.push('--glob', g);
  for (const g of globs(opts.exclude)) args.push('--glob', `!${g}`);
  args.push('--', q, '.');

  return new Promise((resolve, reject) => {
    const child = spawn('rg', args, { cwd, env: childEnv(), stdio: ['ignore', 'pipe', 'pipe'] });
    const byPath = new Map<string, SearchFile>();
    let total = 0;
    let truncated = false;
    let buf = '';
    let err = '';
    const timer = setTimeout(() => {
      truncated = true;
      child.kill();
    }, TIMEOUT_MS);
    const onLine = (line: string) => {
      if (!line || truncated) return;
      let m: { type: string; data: { path?: { text?: string }; lines?: { text?: string }; line_number?: number; submatches?: { start: number; end: number }[] } };
      try {
        m = JSON.parse(line);
      } catch {
        return;
      }
      if (m.type !== 'match') return;
      const path = (m.data.path?.text ?? '').replace(/^\.\//, '');
      const raw = (m.data.lines?.text ?? '').replace(/\r?\n$/, '');
      const text = raw.length > MAX_LINE ? raw.slice(0, MAX_LINE) : raw;
      const ranges = (m.data.submatches ?? [])
        .map((s) => [utf16(raw, s.start), utf16(raw, s.end)] as [number, number])
        .filter(([a]) => a < text.length)
        .map(([a, b]) => [a, Math.min(b, text.length)] as [number, number]);
      let f = byPath.get(path);
      if (!f) byPath.set(path, (f = { path, matches: [] }));
      f.matches.push({ line: m.data.line_number ?? 0, text, ranges });
      if (++total >= MAX_MATCHES) {
        truncated = true;
        child.kill();
      }
    };
    child.stdout.setEncoding('utf8');
    child.stdout.on('data', (chunk: string) => {
      buf += chunk;
      let i: number;
      while ((i = buf.indexOf('\n')) >= 0) {
        onLine(buf.slice(0, i));
        buf = buf.slice(i + 1);
      }
    });
    child.stderr.on('data', (d) => (err += String(d)));
    child.on('error', (e) => {
      clearTimeout(timer);
      reject(new HttpError(500, `Search failed: ${e.message}`));
    });
    child.on('close', (code) => {
      clearTimeout(timer);
      onLine(buf);
      // 0 — matches, 1 — nothing found, killed — limit reached; 2 — a real error (bad glob…)
      if (code === 2 && !truncated && !total) return reject(new HttpError(400, err.trim().split('\n')[0] || 'Search failed'));
      resolve({ files: [...byPath.values()], total, truncated });
    });
  });
}

const escapeRe = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

/** Glob → RegExp for the fallback (supports *, ** and ?; a pattern without / matches the file name anywhere). */
function globRe(g: string): RegExp {
  let re = '';
  for (let i = 0; i < g.length; i++) {
    const c = g[i]!;
    if (c === '*' && g[i + 1] === '*') {
      re += '.*';
      i++;
      if (g[i + 1] === '/') i++;
    } else if (c === '*') re += '[^/]*';
    else if (c === '?') re += '[^/]';
    else re += escapeRe(c);
  }
  return new RegExp(g.includes('/') ? `^${re}$` : `(^|/)${re}(/|$)`);
}

async function searchNode(cwd: string, q: string, opts: SearchOptions): Promise<SearchResult> {
  const source = opts.regex ? q : escapeRe(q);
  const re = new RegExp(opts.word ? `\\b(?:${source})\\b` : source, opts.caseSensitive ? 'g' : 'gi');
  const inc = globs(opts.include).map(globRe);
  const exc = globs(opts.exclude).map(globRe);
  const files: SearchFile[] = [];
  let total = 0;
  const started = Date.now();
  for (const path of await listFiles(cwd)) {
    if (inc.length && !inc.some((r) => r.test(path))) continue;
    if (exc.some((r) => r.test(path))) continue;
    if (Date.now() - started > TIMEOUT_MS) return { files, total, truncated: true };
    let content: Buffer;
    try {
      const abs = join(cwd, path);
      if (statSync(abs).size > 2 * 1024 * 1024) continue;
      content = readFileSync(abs);
    } catch {
      continue;
    }
    if (content.subarray(0, 8000).includes(0)) continue;
    const lines = content.toString('utf8').split('\n');
    let f: SearchFile | null = null;
    for (let n = 0; n < lines.length; n++) {
      const raw = lines[n]!.replace(/\r$/, '');
      const text = raw.slice(0, MAX_LINE);
      const ranges: [number, number][] = [];
      re.lastIndex = 0;
      for (let m = re.exec(raw); m; m = re.exec(raw)) {
        if (m[0] === '') {
          re.lastIndex++;
          continue;
        }
        if (m.index < text.length) ranges.push([m.index, Math.min(m.index + m[0].length, text.length)]);
      }
      if (!ranges.length) continue;
      if (!f) files.push((f = { path, matches: [] }));
      f.matches.push({ line: n + 1, text, ranges });
      if (++total >= MAX_MATCHES) return { files, total, truncated: true };
      if (f.matches.length >= MAX_PER_FILE) break;
    }
  }
  return { files, total, truncated: false };
}

/** For tests: force the fallback engine. */
export const _searchNode = searchNode;
