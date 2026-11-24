import { spawn, type ChildProcess } from 'node:child_process';
import { readdirSync, statSync, watch, type FSWatcher } from 'node:fs';
import { join, resolve } from 'node:path';

/**
 * Dev runner (`pnpm dev`): runs the server and restarts it on any change in src.
 *
 * Replaces `node --watch`, which in practice missed edits made while it was restarting the
 * server (the old code kept running, new routes answered 404) and ignored mtime-only
 * changes (`touch`). Here a change during a restart is remembered and causes one more
 * restart, and every file event counts. One non-recursive watch per directory (works the
 * same on Linux and macOS for a tree this small).
 */

const SRC = resolve(import.meta.dirname);
const ENTRY = join(SRC, 'index.ts');
const DEBOUNCE_MS = 120;
const KILL_AFTER_MS = 5000;

let child: ChildProcess | null = null;
let restarting = false;
let pending = false;
let timer: ReturnType<typeof setTimeout> | null = null;

function start() {
  child = spawn(process.execPath, ['--disable-warning=ExperimentalWarning', ENTRY], { stdio: 'inherit', env: process.env });
  const me = child;
  me.on('exit', (code, signal) => {
    if (child === me) child = null;
    if (!restarting && code !== 0 && signal !== 'SIGTERM') console.log(`[dev] server exited (${code ?? signal}); waiting for changes`);
  });
}

function stop(): Promise<void> {
  const c = child;
  if (!c || c.exitCode !== null || c.signalCode !== null) return Promise.resolve();
  return new Promise((done) => {
    const kill = setTimeout(() => c.kill('SIGKILL'), KILL_AFTER_MS);
    c.once('exit', () => {
      clearTimeout(kill);
      done();
    });
    c.kill('SIGTERM');
  });
}

async function restart(why: string) {
  if (restarting) {
    pending = true;
    return;
  }
  restarting = true;
  console.log(`[dev] ${why} — restarting`);
  await stop();
  start();
  restarting = false;
  if (pending) {
    pending = false;
    void restart('changed during restart');
  }
}

// ---------- watching ----------

const watchers = new Map<string, FSWatcher>();
function watchDir(dir: string) {
  if (watchers.has(dir)) return;
  try {
    const w = watch(dir, { persistent: true }, (_type, name) => {
      const file = name ? join(dir, name.toString()) : dir;
      try {
        if (statSync(file).isDirectory()) watchDir(file);
      } catch {}
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => void restart(file.slice(SRC.length + 1) || 'src'), DEBOUNCE_MS);
    });
    w.on('error', () => {
      w.close();
      watchers.delete(dir);
    });
    watchers.set(dir, w);
  } catch {
    return;
  }
  for (const d of readdirSync(dir, { withFileTypes: true })) if (d.isDirectory()) watchDir(join(dir, d.name));
}

watchDir(SRC);
start();

for (const sig of ['SIGINT', 'SIGTERM'] as const)
  process.on(sig, () => {
    for (const w of watchers.values()) w.close();
    void stop().then(() => process.exit(0));
  });
