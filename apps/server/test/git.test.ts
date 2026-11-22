import { makeRepo, tmp } from './setup.ts';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import * as git from '../src/git.ts';

const sh = (cwd: string, ...args: string[]) => execFileSync('git', ['-C', cwd, ...args], { encoding: 'utf8' }).trim();

function repo(name: string) {
  const dir = makeRepo(join(tmp, name));
  sh(dir, 'config', 'user.email', 't@example.com');
  sh(dir, 'config', 'user.name', 'T');
  writeFileSync(join(dir, 'a.txt'), 'one\n');
  sh(dir, 'add', '-A');
  sh(dir, 'commit', '-qm', 'first');
  return dir;
}

test('git log: parents and refs for the graph, limit with "more"', async () => {
  const dir = repo('git-log');
  sh(dir, 'switch', '-qc', 'feat');
  writeFileSync(join(dir, 'b.txt'), 'b\n');
  sh(dir, 'add', '-A');
  sh(dir, 'commit', '-qm', 'feat work');
  sh(dir, 'switch', '-q', 'main');
  sh(dir, 'merge', '-q', '--no-ff', '--no-edit', 'feat');
  const log = await git.log(dir, 10);
  assert.equal(log.commits.length, 3);
  assert.equal(log.more, false);
  assert.equal(log.commits[0].parents.length, 2, 'merge commit has two parents');
  assert.ok(log.commits[0].refs.includes('HEAD -> main'));
  assert.equal((await git.log(dir, 2)).more, true);
});

test('git ops: stash, discard, reset, branch at commit, delete', async () => {
  const dir = repo('git-ops');
  const first = sh(dir, 'rev-parse', 'HEAD');
  writeFileSync(join(dir, 'a.txt'), 'two\n');
  writeFileSync(join(dir, 'new.txt'), 'new\n');
  assert.ok((await git.ops.stashPush(dir, 'wip', true)).ok);
  let st = await git.status(dir);
  assert.equal(st.files.length, 0);
  assert.equal(st.stashes, 1);
  const list = await git.stashes(dir);
  assert.equal(list[0].ref, 'stash@{0}');
  assert.equal(list[0].message, 'wip');
  assert.match(await git.show(dir, 'stash@{0}'), /two/);
  assert.ok((await git.ops.stash(dir, 'pop', 'stash@{0}')).ok);
  assert.equal((await git.status(dir)).files.length, 2);

  assert.ok((await git.ops.discard(dir, ['a.txt', 'new.txt'])).ok);
  assert.equal(readFileSync(join(dir, 'a.txt'), 'utf8'), 'one\n');
  assert.equal(existsSync(join(dir, 'new.txt')), false);

  writeFileSync(join(dir, 'a.txt'), 'three\n');
  sh(dir, 'commit', '-qam', 'second');
  assert.ok((await git.ops.reset(dir, first, 'soft')).ok);
  st = await git.status(dir);
  assert.equal(st.files[0]?.index, 'M', 'soft reset keeps the change staged');
  assert.ok((await git.ops.reset(dir, first, 'hard')).ok);
  assert.equal((await git.status(dir)).files.length, 0);

  assert.ok((await git.ops.branchAt(dir, 'from-first', first)).ok);
  assert.equal((await git.status(dir)).branch, 'from-first');
  sh(dir, 'switch', '-q', 'main');
  assert.ok((await git.ops.deleteBranch(dir, 'from-first', false)).ok);

  assert.equal((await git.ops.reset(dir, '--hard', 'hard')).ok, false, 'options are not revisions');
  assert.equal((await git.ops.merge(dir, 'a..b', false)).ok, false);
});

test('git merge conflict: state, refuse continue, abort', async () => {
  const dir = repo('git-conflict');
  sh(dir, 'switch', '-qc', 'other');
  writeFileSync(join(dir, 'a.txt'), 'other\n');
  sh(dir, 'commit', '-qam', 'other');
  sh(dir, 'switch', '-q', 'main');
  writeFileSync(join(dir, 'a.txt'), 'main\n');
  sh(dir, 'commit', '-qam', 'main');
  assert.equal((await git.ops.merge(dir, 'other', false)).ok, false);
  let st = await git.status(dir);
  assert.equal(st.state, 'merge');
  assert.ok(st.files.some((f) => f.conflicted));
  assert.equal((await git.ops.proceed(dir, 'continue')).ok, false);
  assert.ok((await git.ops.proceed(dir, 'abort')).ok);
  st = await git.status(dir);
  assert.equal(st.state, null);
  assert.equal(readFileSync(join(dir, 'a.txt'), 'utf8'), 'main\n');

  // rebase with a conflict, resolve, continue without an editor
  assert.equal((await git.ops.rebase(dir, 'other')).ok, false);
  assert.equal((await git.status(dir)).state, 'rebase');
  writeFileSync(join(dir, 'a.txt'), 'resolved\n');
  sh(dir, 'add', 'a.txt');
  assert.ok((await git.ops.proceed(dir, 'continue')).ok);
  assert.equal((await git.status(dir)).state, null);
});
