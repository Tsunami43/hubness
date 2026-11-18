import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmodSync, closeSync, existsSync, lstatSync, mkdirSync, openSync, readFileSync, readdirSync, readSync, realpathSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { basename, dirname, join, relative, resolve, sep } from 'node:path';
import type { BrowseResponse, FileContent, FsEntry } from '@hubness/shared';
import { HttpError } from './errors.ts';

const HIDDEN_IN_TREE = new Set(['.git']);
/** Outside a git repository, the usual build and dependency folders are treated as internal. */
const IGNORED_FALLBACK = new Set(['node_modules', 'target', 'dist', '.next', '.venv', 'venv', '__pycache__', '.cache']);
const MAX_FILE = 512 * 1024;

/** A path inside the project root; escaping the root (including via symlinks) is forbidden. */
export function safeJoin(root: string, rel: string): string {
  const rootReal = realpathSync(root);
  const target = resolve(rootReal, rel || '.');
  const real = existsSync(target) ? realpathSync(target) : target;
  if (real !== rootReal && !real.startsWith(rootReal + sep)) throw new HttpError(400, 'Path is outside the project');
  return real;
}

const byDirThenName = (a: FsEntry, b: FsEntry) =>
  a.dir === b.dir ? a.name.localeCompare(b.name) : a.dir ? -1 : 1;

/** Which paths git ignores (`git check-ignore`); null — the folder is not in a repository. */
function gitIgnored(cwd: string, paths: string[]): Set<string> | null {
  if (!paths.length) return new Set();
  try {
    const out = execFileSync('git', ['-C', cwd, 'check-ignore', '--stdin'], { input: paths.join('\n'), encoding: 'utf8', stdio: ['pipe', 'pipe', 'ignore'] });
    return new Set(out.split('\n').filter(Boolean));
  } catch (e) {
    const err = e as { status?: number; stdout?: string };
    // 1 — nothing is ignored; 128 — not a repository
    if (err.status === 1) return new Set();
    return null;
  }
}

export function listTree(root: string, rel: string): FsEntry[] {
  const abs = safeJoin(root, rel);
  const rootReal = realpathSync(root);
  const entries = readdirSync(abs, { withFileTypes: true })
    .filter((d) => !HIDDEN_IN_TREE.has(d.name))
    .map((d) => {
      const p = join(abs, d.name);
      let dir = d.isDirectory();
      let size: number | undefined;
      try {
        const s = statSync(p);
        dir = s.isDirectory();
        size = dir ? undefined : s.size;
      } catch {}
      return { name: d.name, path: relative(rootReal, p), dir, size } as FsEntry;
    })
    .sort(byDirThenName);
  const ignored = gitIgnored(abs, entries.map((e) => (e.dir ? `${e.name}/` : e.name)));
  for (const e of entries) {
    e.ignored = ignored ? ignored.has(e.dir ? `${e.name}/` : e.name) || ignored.has(e.name) : IGNORED_FALLBACK.has(e.name);
  }
  return entries;
}

export function readFileSafe(root: string, rel: string): FileContent {
  const abs = safeJoin(root, rel);
  let st;
  try {
    st = statSync(abs);
  } catch {
    throw new HttpError(404, 'File not found');
  }
  if (st.isDirectory()) throw new HttpError(400, 'This is a folder');
  const len = Math.min(st.size, MAX_FILE);
  const buf = Buffer.alloc(len);
  const fd = openSync(abs, 'r');
  try {
    readSync(fd, buf, 0, len, 0);
  } finally {
    closeSync(fd);
  }
  const binary = buf.subarray(0, 8000).includes(0);
  const truncated = st.size > MAX_FILE;
  return {
    path: rel,
    size: st.size,
    binary,
    truncated,
    content: binary ? '' : buf.toString('utf8'),
    hash: binary || truncated ? undefined : sha256(buf),
  };
}

const sha256 = (b: Buffer | string) => createHash('sha256').update(b).digest('hex');

/**
 * Save a text file from the editor. Only existing regular files inside the root, never
 * inside .git, text only and no larger than what the viewer can load. With baseHash the save
 * is refused (409) if the file changed since it was opened — an agent may be editing it too.
 * The write is atomic (temp file + rename) and keeps the file mode.
 */
export function writeFileSafe(root: string, rel: string, content: string, baseHash?: string): FileContent {
  if (typeof content !== 'string') throw new HttpError(400, 'content must be a string');
  const abs = safeJoin(root, rel);
  const inside = relative(realpathSync(root), abs);
  if (!inside || inside.split(sep).includes('.git')) throw new HttpError(400, 'This path can’t be edited');
  if (!existsSync(abs)) throw new HttpError(404, 'File not found');
  const st = statSync(abs);
  if (!st.isFile()) throw new HttpError(400, 'Not a regular file');
  const bytes = Buffer.from(content, 'utf8');
  if (bytes.length > MAX_FILE) throw new HttpError(413, 'The file is too large to edit here');
  if (content.includes('\0')) throw new HttpError(400, 'Binary content is not allowed');
  const current = readFileSync(abs);
  if (current.subarray(0, 8000).includes(0)) throw new HttpError(400, 'Binary files can’t be edited');
  if (baseHash && sha256(current) !== baseHash) throw new HttpError(409, 'The file changed on disk since you opened it');
  const tmp = join(dirname(abs), `.${basename(abs)}.hubness-${process.pid}-${Date.now()}.tmp`);
  try {
    writeFileSync(tmp, bytes, { mode: st.mode & 0o7777 });
    chmodSync(tmp, st.mode & 0o7777);
    renameSync(tmp, abs);
  } catch (e) {
    rmSync(tmp, { force: true });
    throw e;
  }
  return readFileSafe(root, rel);
}

// ---------- file operations (Explorer: new / rename / move / delete) ----------

/**
 * An entry path inside the project, checked lexically for the entry itself (a symlink is
 * renamed/deleted as a link, never its target) and by realpath for its parent folder.
 * Never the root itself, never inside .git.
 */
function entryPath(root: string, rel: string): string {
  const rootReal = realpathSync(root);
  const clean = (rel ?? '').replace(/\\/g, '/').replace(/^\/+|\/+$/g, '');
  if (!clean || clean.split('/').some((p) => p === '' || p === '.' || p === '..')) throw new HttpError(400, 'Invalid path');
  if (clean.split('/').includes('.git')) throw new HttpError(400, 'This path can’t be changed');
  const abs = resolve(rootReal, clean);
  let parent = dirname(abs);
  // the parent may not exist yet (new nested path): check the nearest existing ancestor
  let probe = parent;
  while (!existsSync(probe)) probe = dirname(probe);
  const probeReal = realpathSync(probe);
  if (probeReal !== rootReal && !probeReal.startsWith(rootReal + sep)) throw new HttpError(400, 'Path is outside the project');
  if (existsSync(parent)) parent = realpathSync(parent);
  return join(parent, basename(abs));
}

const exists = (p: string) => {
  try {
    lstatSync(p);
    return true;
  } catch {
    return false;
  }
};

/** New empty file or folder (parent folders are created). */
export function createEntry(root: string, rel: string, dir: boolean): { path: string } {
  const abs = entryPath(root, rel);
  if (exists(abs)) throw new HttpError(409, 'A file or folder with this name already exists');
  mkdirSync(dirname(abs), { recursive: true });
  if (dir) mkdirSync(abs);
  else writeFileSync(abs, '', { flag: 'wx' });
  return { path: relative(realpathSync(root), abs) };
}

/** Rename or move a file/folder inside the project; never overwrites. */
export function renameEntry(root: string, from: string, to: string): { path: string } {
  const src = entryPath(root, from);
  const dst = entryPath(root, to);
  if (!exists(src)) throw new HttpError(404, 'File not found');
  if (src === dst) return { path: relative(realpathSync(root), dst) };
  if (exists(dst)) throw new HttpError(409, 'A file or folder with this name already exists');
  if (dst.startsWith(src + sep)) throw new HttpError(400, 'Can’t move a folder into itself');
  mkdirSync(dirname(dst), { recursive: true });
  renameSync(src, dst);
  return { path: relative(realpathSync(root), dst) };
}

/** Delete a file or a folder (recursively). Git keeps tracked files recoverable. */
export function deleteEntry(root: string, rel: string): void {
  const abs = entryPath(root, rel);
  if (!exists(abs)) throw new HttpError(404, 'File not found');
  rmSync(abs, { recursive: true, force: false });
}

/** Browse server folders — for picking a project folder. */
export function browse(path?: string): BrowseResponse {
  const abs = resolve(path || homedir());
  if (!existsSync(abs) || !statSync(abs).isDirectory()) throw new HttpError(400, 'Folder not found');
  let entries: FsEntry[] = [];
  try {
    entries = readdirSync(abs, { withFileTypes: true })
      .filter((d) => d.isDirectory() && !d.name.startsWith('.') && d.name !== 'node_modules')
      .map((d) => ({ name: d.name, path: join(abs, d.name), dir: true }))
      .sort(byDirThenName);
  } catch {
    throw new HttpError(403, 'No access to the folder');
  }
  const parent = dirname(abs);
  return { path: abs, parent: parent === abs ? null : parent, entries, isGitRepo: existsSync(join(abs, '.git')) };
}

/** All project files for search: git ls-files (tracked + untracked, excluding ignored); outside git — folder walk. */
export function listFiles(root: string, limit = 20_000): string[] {
  const rootReal = realpathSync(root);
  try {
    const out = execFileSync('git', ['-C', rootReal, 'ls-files', '-co', '--exclude-standard'], {
      encoding: 'utf8',
      maxBuffer: 64 * 1024 * 1024,
      stdio: ['ignore', 'pipe', 'ignore'],
    });
    return out.split('\n').filter(Boolean).slice(0, limit);
  } catch {
    const res: string[] = [];
    const walk = (dir: string) => {
      if (res.length >= limit) return;
      for (const d of readdirSync(dir, { withFileTypes: true })) {
        if (HIDDEN_IN_TREE.has(d.name) || IGNORED_FALLBACK.has(d.name)) continue;
        const p = join(dir, d.name);
        if (d.isDirectory()) walk(p);
        else res.push(relative(rootReal, p));
        if (res.length >= limit) return;
      }
    };
    walk(rootReal);
    return res;
  }
}
