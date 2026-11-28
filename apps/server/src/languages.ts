import { execFile } from 'node:child_process';
import { stat } from 'node:fs/promises';
import { join } from 'node:path';
import { promisify } from 'node:util';
import * as linguist from 'linguist-languages';
import { SAFE_GIT_ARGS } from './git.ts';

/**
 * Repository languages — like the language bar on GitHub (GitHub Linguist data:
 * extensions, file names, colors). We count bytes of git-tracked files;
 * like Linguist, we count only programming and markup languages,
 * skipping vendor/build/documentation. Ambiguous extensions are resolved by a
 * preference table (Linguist resolves them with content heuristics).
 */

export interface RepoLanguage {
  name: string;
  color: string | null;
  percent: number;
}

const run = promisify(execFile);

type Lang = { name: string; type: string; color?: string; extensions?: string[]; filenames?: string[] };
const LANGS = Object.values(linguist as unknown as Record<string, Lang>).filter((l) => l && typeof l === 'object' && 'type' in l);

const PREFER: Record<string, string> = {
  '.ts': 'TypeScript',
  '.h': 'C',
  '.m': 'Objective-C',
  '.pl': 'Perl',
  '.pm': 'Perl',
  '.r': 'R',
  '.cs': 'C#',
  '.fs': 'F#',
  '.rs': 'Rust',
  '.sql': 'SQL',
  '.md': 'Markdown',
  '.v': 'Verilog',
  '.inc': 'PHP',
  '.cls': 'Apex',
  '.l': 'Lex',
  '.mod': 'Modula-2',
  '.toc': 'TeX',
  '.html': 'HTML',
  '.json': 'JSON',
  '.yml': 'YAML',
  '.yaml': 'YAML',
};

const byExt = new Map<string, Lang>();
const byName = new Map<string, Lang>();
const rank = (l: Lang) => (l.type === 'programming' ? 0 : l.type === 'markup' ? 1 : 2);
for (const l of LANGS) {
  for (const e of l.extensions ?? []) {
    const key = e.toLowerCase();
    const cur = byExt.get(key);
    if (!cur || rank(l) < rank(cur)) byExt.set(key, l);
  }
  for (const f of l.filenames ?? []) byName.set(f, l);
}
for (const [ext, name] of Object.entries(PREFER)) {
  const l = LANGS.find((x) => x.name === name);
  if (l) byExt.set(ext, l);
}

// like Linguist's vendor.yml/documentation.yml (abridged)
const SKIP = /(^|\/)(node_modules|vendor|third[_-]party|dist|build|out|target|\.yarn|bower_components|Pods|docs?|documentation)\/|\.min\.(js|css)$|(^|\/)(package-lock\.json|pnpm-lock\.yaml|yarn\.lock|Cargo\.lock|go\.sum|composer\.lock|Gemfile\.lock|poetry\.lock)$/i;

function langOf(path: string): Lang | undefined {
  const name = path.slice(path.lastIndexOf('/') + 1);
  const exact = byName.get(name);
  if (exact) return exact;
  const lower = name.toLowerCase();
  // compound extensions (.d.ts, .blade.php) — from longest to shortest
  for (let i = lower.indexOf('.'); i >= 0; i = lower.indexOf('.', i + 1)) {
    const l = byExt.get(lower.slice(i));
    if (l) return l;
  }
  return undefined;
}

const cache = new Map<string, { head: string; langs: RepoLanguage[] }>();
const MAX_FILES = 50_000;

export async function repoLanguages(dir: string): Promise<RepoLanguage[]> {
  let head = '';
  try {
    head = (await run('git', ['-C', dir, 'rev-parse', 'HEAD'], { timeout: 5000 })).stdout.trim();
  } catch {
    // empty repository without commits — count from the working tree
  }
  const hit = cache.get(dir);
  if (hit && head && hit.head === head) return hit.langs;
  let files: string[] = [];
  try {
    const out = await run('git', [...SAFE_GIT_ARGS, '-C', dir, 'ls-files', '-z', '--cached', '--others', '--exclude-standard'], { timeout: 10_000, maxBuffer: 64 * 1024 * 1024 });
    files = out.stdout.split('\0').filter(Boolean).slice(0, MAX_FILES);
  } catch {
    return [];
  }
  const bytes = new Map<Lang, number>();
  const BATCH = 256;
  for (let i = 0; i < files.length; i += BATCH) {
    await Promise.all(
      files.slice(i, i + BATCH).map(async (f) => {
        if (SKIP.test(f)) return;
        const l = langOf(f);
        if (!l || (l.type !== 'programming' && l.type !== 'markup')) return;
        try {
          const s = await stat(join(dir, f));
          if (s.isFile()) bytes.set(l, (bytes.get(l) ?? 0) + s.size);
        } catch {}
      }),
    );
  }
  const total = [...bytes.values()].reduce((a, b) => a + b, 0);
  const langs = total
    ? [...bytes]
        .sort((a, b) => b[1] - a[1])
        .map(([l, b]) => ({ name: l.name, color: l.color ?? null, percent: Math.round((b / total) * 1000) / 10 }))
        .filter((l) => l.percent >= 0.1)
    : [];
  if (head) cache.set(dir, { head, langs });
  return langs;
}
