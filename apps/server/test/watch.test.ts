import { makeRepo, tmp } from './setup.ts';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { FileChangeMsg } from '@hubness/shared';
import { watchFolder, watchedFolders } from '../src/watch.ts';

type Msg = Omit<FileChangeMsg, 'projectId'>;

/** Collect batches until `done` holds (or time out). */
function until(root: string, act: () => void, done: (all: Msg[]) => boolean, ms = 3000): Promise<Msg[]> {
  return new Promise((resolve, reject) => {
    const all: Msg[] = [];
    const timer = setTimeout(() => (off(), reject(new Error(`no matching change: ${JSON.stringify(all)}`))), ms);
    const off = watchFolder(root, (m) => {
      all.push(m);
      if (done(all)) {
        clearTimeout(timer);
        off();
        resolve(all);
      }
    });
    // give the watches a moment to attach
    setTimeout(act, 50);
  });
}
const has = (all: Msg[], p: string) => all.some((m) => m.overflow || m.paths.includes(p));

test('file edits, new folders and commits are reported; the watcher stops after the last subscriber', async () => {
  const root = makeRepo(join(tmp, 'watched'));
  writeFileSync(join(root, 'main.py'), 'print(1)\n');

  await until(root, () => writeFileSync(join(root, 'main.py'), 'print(2)\n'), (all) => has(all, 'main.py'));

  // a folder created after the watch started is watched too
  await until(
    root,
    () => {
      mkdirSync(join(root, 'pkg'));
      setTimeout(() => writeFileSync(join(root, 'pkg', 'tree.py'), 'x = 1\n'), 100);
    },
    (all) => has(all, 'pkg/tree.py'),
  );

  const git = (...args: string[]) => execFileSync('git', ['-C', root, '-c', 'user.email=t@t', '-c', 'user.name=t', ...args]);
  await until(root, () => (git('add', '.'), git('commit', '-qm', 'init')), (all) => all.some((m) => m.git));

  assert.equal(watchedFolders(), 0, 'no subscribers — no watcher');
});
