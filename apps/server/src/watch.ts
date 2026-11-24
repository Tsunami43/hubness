import { existsSync, readdirSync, statSync, watch, type FSWatcher } from 'node:fs';
import { join, relative, sep } from 'node:path';
import type { FileChangeMsg } from '@hubness/shared';
import { PLATFORM } from './system/host.ts';

/**
 * Live file changes of project folders, so open editors, the file tree and git state follow
 * what agents, shells and other people do on disk.
 *
 * One watcher per folder, shared by every subscriber: started with the first one, stopped
 * after the last. Events are batched (BATCH_MS) per folder.
 * - Linux: one inotify watch per directory (non-recursive fs.watch), skipping heavy and
 *   generated folders — Node's recursive mode would also walk node_modules and can exhaust
 *   the inotify limit. New directories are picked up as they appear.
 * - macOS: FSEvents through recursive fs.watch (one handle per folder), filtered the same way.
 * .git is not walked: changes of HEAD, index and refs are reported as `git: true`.
 */

const BATCH_MS = 150;
/** More changed paths than this in one batch → `overflow` (the client reloads everything). */
const MAX_PATHS = 200;
/** Directory watches per folder on Linux; past it deeper changes are not reported. */
const MAX_DIRS = 4000;
const SKIP = new Set(['.git', 'node_modules', 'target', 'dist', 'build', '.next', '.nuxt', '.venv', 'venv', '__pycache__', '.cache', '.turbo', '.gradle', '.mypy_cache', '.pytest_cache', '.ruff_cache', 'coverage']);
const GIT_FILE = /^(HEAD|ORIG_HEAD|FETCH_HEAD|MERGE_HEAD|index|packed-refs)$|^refs[\\/]/;

type Listener = (msg: Omit<FileChangeMsg, 'projectId'>) => void;

interface Entry {
  listeners: Set<Listener>;
  stop: () => void;
}

const folders = new Map<string, Entry>();

/** Subscribe to changes in a folder (absolute path). Returns the unsubscribe function. */
export function watchFolder(root: string, fn: Listener): () => void {
  let entry = folders.get(root);
  if (!entry) {
    const listeners = new Set<Listener>();
    entry = { listeners, stop: start(root, (msg) => listeners.forEach((l) => l(msg))) };
    folders.set(root, entry);
  }
  entry.listeners.add(fn);
  const e = entry;
  return () => {
    e.listeners.delete(fn);
    if (e.listeners.size || folders.get(root) !== e) return;
    folders.delete(root);
    e.stop();
  };
}

/** Number of folders being watched (for tests). */
export const watchedFolders = () => folders.size;

function start(root: string, emit: Listener): () => void {
  const paths = new Set<string>();
  let git = false;
  let overflow = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  const flush = () => {
    timer = null;
    if (!paths.size && !git && !overflow) return;
    const msg = overflow ? { paths: [], git: true, overflow: true } : { paths: [...paths], git };
    paths.clear();
    git = overflow = false;
    emit(msg);
  };
  const push = (rel: string) => {
    if (!rel || rel.startsWith('..')) return;
    const p = rel.split(sep).join('/');
    if (paths.size >= MAX_PATHS) overflow = true;
    else paths.add(p);
    timer ??= setTimeout(flush, BATCH_MS);
  };
  const pushGit = () => {
    git = true;
    timer ??= setTimeout(flush, BATCH_MS);
  };

  const stopTree = PLATFORM === 'darwin' ? recursiveWatch(root, push, pushGit) : dirWatches(root, push);
  const stopGit = PLATFORM === 'darwin' ? () => {} : gitWatch(root, pushGit);
  return () => {
    if (timer) clearTimeout(timer);
    stopTree();
    stopGit();
  };
}

/** Linux (and other non-macOS): a non-recursive watch per directory. */
function dirWatches(root: string, push: (rel: string) => void): () => void {
  const watchers = new Map<string, FSWatcher>();
  const remove = (dir: string) => {
    for (const [d, w] of watchers)
      if (d === dir || d.startsWith(dir + sep)) {
        w.close();
        watchers.delete(d);
      }
  };
  const add = (dir: string) => {
    if (watchers.size >= MAX_DIRS || watchers.has(dir)) return;
    let w: FSWatcher;
    try {
      w = watch(dir, { persistent: false }, (_type, name) => {
        if (!name) return push(relative(root, dir));
        const full = join(dir, name.toString());
        push(relative(root, full));
        if (SKIP.has(name.toString())) return;
        // a new directory gets its own watch; a removed one drops its watches
        let isDir = false;
        try {
          isDir = statSync(full).isDirectory();
        } catch {
          return remove(full);
        }
        if (isDir && !watchers.has(full)) add(full);
      });
    } catch {
      return;
    }
    w.on('error', () => remove(dir));
    watchers.set(dir, w);
    let list;
    try {
      list = readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const d of list) if (d.isDirectory() && !SKIP.has(d.name)) add(join(dir, d.name));
  };
  add(root);
  return () => {
    for (const w of watchers.values()) w.close();
    watchers.clear();
  };
}

/** The repository's .git folder and its refs: commits, checkouts, staging. */
function gitWatch(root: string, pushGit: () => void): () => void {
  const gitDir = join(root, '.git');
  const ws: FSWatcher[] = [];
  try {
    if (!existsSync(gitDir) || !statSync(gitDir).isDirectory()) return () => {};
  } catch {
    return () => {};
  }
  const on = (prefix: string) => (_t: string, name: string | Buffer | null) => {
    if (name && GIT_FILE.test(prefix + name.toString())) pushGit();
  };
  for (const [dir, prefix] of [
    [gitDir, ''],
    [join(gitDir, 'refs', 'heads'), 'refs/'],
    [join(gitDir, 'refs', 'remotes'), 'refs/'],
  ] as const) {
    try {
      const w = watch(dir, { persistent: false }, on(prefix));
      w.on('error', () => w.close());
      ws.push(w);
    } catch {}
  }
  return () => ws.forEach((w) => w.close());
}

/** macOS: one recursive FSEvents watch; skipped folders and .git are filtered here. */
function recursiveWatch(root: string, push: (rel: string) => void, pushGit: () => void): () => void {
  let w: FSWatcher;
  try {
    w = watch(root, { persistent: false, recursive: true }, (_t, name) => {
      if (!name) return;
      const rel = name.toString();
      const parts = rel.split(sep);
      if (parts[0] === '.git') {
        if (GIT_FILE.test(parts.slice(1).join('/'))) pushGit();
        return;
      }
      // inside a skipped folder: report only the folder itself (it appeared or vanished)
      const skipAt = parts.findIndex((p) => SKIP.has(p));
      push(skipAt >= 0 ? parts.slice(0, skipAt + 1).join(sep) : rel);
    });
  } catch {
    return () => {};
  }
  w.on('error', () => w.close());
  return () => w.close();
}
