import { makeRepo, tmp } from './setup.ts';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { chmodSync, existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { Hono } from 'hono';
import { projects, secretRows } from '../src/db/index.ts';
import { decrypt, deleteSecret, encrypt, listSecrets, resetKeys, secretEnv, setSecret, unreadableSecrets } from '../src/secrets.ts';
import { createRun, listRuns, suggestRuns, updateRun, deleteRun } from '../src/runs.ts';
import { bwrapArgs, execCommand, runtimeOf, setRuntime } from '../src/environments.ts';
import { csrfGuard } from '../src/csrf.ts';
import { childEnv, config } from '../src/config.ts';
import { closePreview, ensurePreview } from '../src/preview.ts';
import { HttpError } from '../src/errors.ts';
import { createEntry, deleteEntry, readFileSafe, renameEntry, writeFileSafe } from '../src/fsops.ts';
import { _searchNode, searchFiles } from '../src/search.ts';
import { ops as gitOps } from '../src/git.ts';
import { runCli } from '../src/agents/kit.ts';
import { runOnce } from '../src/terminal.ts';
import { sh } from '../src/system/exec.ts';
import { execFileSync } from 'node:child_process';

const repo = makeRepo(join(tmp, 'shop', 'web'));
const other = makeRepo(join(tmp, 'shop', 'billing'));
const p = projects.create('web', repo, 'normal');
const o = projects.create('billing', other, 'normal');

const status = (fn: () => unknown) => {
  try {
    fn();
  } catch (e) {
    return e instanceof HttpError ? e.status : -1;
  }
  return 0;
};

// ---------- secrets ----------

test('secrets are encrypted at rest, bound to project and name, never listed with values', () => {
  setSecret(p.id, 'STRIPE_KEY', 'sk_test_123');
  const row = secretRows.list(p.id)[0]!;
  assert.ok(!row.value.includes('sk_test_123'), 'stored encrypted');
  assert.match(row.value, /^v1:[0-9a-f]{8}:/, 'carries a key id');
  assert.deepEqual(Object.keys(listSecrets(p.id)[0]!).sort(), ['name', 'updatedAt'], 'no value in the listing');
  assert.equal(secretEnv(p.id).STRIPE_KEY, 'sk_test_123');
  assert.throws(() => decrypt(o.id, 'STRIPE_KEY', row.value), 'another project cannot decrypt it');
  assert.throws(() => decrypt(p.id, 'OTHER', row.value), 'another name cannot decrypt it');
  assert.equal(statSync(join(config.dataDir, 'secret.key')).mode & 0o777, 0o600, 'generated key file is private');
});

test('secret names are validated; reserved and hub variables are refused', () => {
  assert.equal(status(() => setSecret(p.id, '1BAD', 'x')), 400);
  assert.equal(status(() => setSecret(p.id, 'PATH', 'x')), 400);
  assert.equal(status(() => setSecret(p.id, 'HUBNESS_SECRET_KEY', 'x')), 400);
  assert.equal(status(() => setSecret(p.id, 'EMPTY', '')), 400);
  deleteSecret(p.id, 'STRIPE_KEY');
  assert.equal(listSecrets(p.id).length, 0);
});

test('key rotation: new key encrypts, old key still decrypts', () => {
  const oldKey = 'a'.repeat(40);
  const newKey = 'b'.repeat(40);
  process.env.HUBNESS_SECRET_KEY = oldKey;
  resetKeys();
  const blob = encrypt(p.id, 'TOKEN', 'v1');
  process.env.HUBNESS_SECRET_KEY = `${newKey},${oldKey}`;
  resetKeys();
  assert.equal(decrypt(p.id, 'TOKEN', blob), 'v1', 'old values stay readable');
  assert.notEqual(encrypt(p.id, 'TOKEN', 'v2').split(':')[1], blob.split(':')[1], 'new values use the new key');
  process.env.HUBNESS_SECRET_KEY = newKey;
  resetKeys();
  secretRows.set(p.id, 'TOKEN', blob);
  assert.deepEqual(unreadableSecrets(p.id), ['TOKEN'], 'without the old key the value is reported, not crashed on');
  assert.deepEqual(secretEnv(p.id), {});
  secretRows.delete(p.id, 'TOKEN');
  delete process.env.HUBNESS_SECRET_KEY;
  resetKeys();
});

// ---------- run ----------

test('run suggestions come from the repository and skip commands already added', () => {
  writeFileSync(join(repo, 'package.json'), JSON.stringify({ scripts: { build: 'vite build', dev: 'vite', test: 'vitest' } }));
  writeFileSync(join(repo, 'pnpm-lock.yaml'), '');
  writeFileSync(join(repo, 'Makefile'), 'migrate:\n\techo ok\nX := 1\n');
  const s = suggestRuns(p);
  assert.deepEqual(s.slice(0, 3).map((x) => x.command), ['pnpm dev', 'pnpm test', 'pnpm build'], 'dev first, package manager from the lock file');
  assert.ok(s.some((x) => x.command === 'make migrate'));
  assert.ok(!s.some((x) => x.command === 'make X'), 'variables are not targets');
  createRun(p, { name: 'dev', command: 'pnpm dev' });
  assert.ok(!suggestRuns(p).some((x) => x.command === 'pnpm dev'));
});

test('run commands: validation, the first one is primary, primary moves on delete', () => {
  assert.equal(status(() => createRun(p, { name: '', command: 'x' })), 400);
  assert.equal(status(() => createRun(p, { name: 'x', command: 'y', port: 70000 })), 400);
  const [dev] = listRuns(p);
  assert.ok(dev!.primary);
  const test2 = createRun(p, { name: 'test', command: 'pnpm test', port: 3000 });
  assert.equal(test2.primary, false);
  assert.equal(test2.activePort, 3000);
  updateRun(p, test2.id, { primary: true });
  assert.deepEqual(listRuns(p).map((r) => r.primary), [false, true]);
  deleteRun(p, test2.id);
  assert.equal(listRuns(p)[0]!.primary, true);
});

// ---------- environment ----------

test('environment settings: validation and defaults', () => {
  assert.equal(runtimeOf(p).environment, 'local');
  assert.equal(status(() => setRuntime(p, { environment: 'vm' })), 400);
  assert.equal(status(() => setRuntime(p, { image: 'bad image!' })), 400);
  assert.equal(setRuntime(p, { shareSecretsWithAgents: true }).shareSecretsWithAgents, true);
  writeFileSync(join(repo, '.devcontainer.json'), '{ // comment\n "image": "mcr.microsoft.com/devcontainers/typescript-node:22", /* x */ "x": "https://a", }');
  assert.equal(runtimeOf(p).image, 'mcr.microsoft.com/devcontainers/typescript-node:22', 'image from devcontainer.json');
});

test('sandbox hides other projects and hub data, exposes only the scope repositories', () => {
  const args = bwrapArgs({ repos: [repo], cwd: repo });
  const pairs = (flag: string) => args.flatMap((a, i) => (a === flag ? [args[i + 1]!] : []));
  assert.ok(pairs('--tmpfs').includes(other), 'other project hidden');
  assert.ok(pairs('--tmpfs').includes(config.dataDir), 'hub data hidden');
  assert.ok(pairs('--bind').includes(repo), 'scope repository writable');
  assert.ok(!pairs('--bind').includes(other));
  assert.ok(args.includes('--unshare-pid') && args.includes('--new-session'));
  assert.ok(args.indexOf(repo) > args.indexOf(other), 'scope repo is mounted after the hiding layers');
});

test('sandboxed commands and shells really start (all mount points exist)', async (t) => {
  const { sandboxAvailable } = await import('../src/environments.ts');
  if (!sandboxAvailable()) return t.skip('bubblewrap is not installed');
  const { spawnSync } = await import('node:child_process');
  setRuntime(p, { sandbox: true });
  for (const [label, cmd] of [
    ['run', 'echo ok-$((1+1)); ls /'],
    ['shell', null],
  ] as const) {
    const spec = execCommand({ kind: 'project', project: p }, cmd, {});
    // shell: exit right away instead of an interactive login; what matters is that bwrap mounted everything
    const script = cmd ? spec.command : spec.command.replace(/exec (\S+) -l'?$/, "exit 0'");
    const r = spawnSync('bash', ['-c', script], { cwd: spec.cwd, env: { ...process.env, ...spec.env }, encoding: 'utf8', timeout: 20_000 });
    assert.equal(r.status, 0, `${label}: ${r.stderr}`);
    if (cmd) assert.match(r.stdout, /ok-2/);
  }
});

test('system project never runs in the sandbox', () => {
  const home = projects.create('home', join(tmp, 'hub-home'), 'system');
  const spec = execCommand({ kind: 'project', project: home }, 'true', {});
  assert.ok(!spec.command.includes('bwrap'));
});

test('child processes do not inherit hub settings, secrets or pnpm variables', () => {
  process.env.HUBNESS_SECRET_KEY = 'x'.repeat(40);
  process.env.npm_config_registry = 'https://example.invalid';
  const env = childEnv();
  assert.equal(env.HUBNESS_SECRET_KEY, undefined);
  assert.equal(env.HUBNESS_DATA_DIR, undefined);
  assert.equal(env.npm_config_registry, undefined);
  assert.ok(env.PATH);
  delete process.env.HUBNESS_SECRET_KEY;
  delete process.env.npm_config_registry;
});

test('spawned tools, CLIs and git hooks do not see hub settings or secrets', async () => {
  const dir = makeRepo(join(tmp, 'env-leak'));
  execFileSync('git', ['-C', dir, 'config', 'user.name', 't']);
  execFileSync('git', ['-C', dir, 'config', 'user.email', 't@t']);
  const seen = join(tmp, 'env-leak-hook.txt');
  writeFileSync(join(dir, '.git/hooks/pre-commit'), `#!/bin/sh\necho "\${HUBNESS_SECRET_KEY-unset}" > ${seen}\n`);
  chmodSync(join(dir, '.git/hooks/pre-commit'), 0o755);
  writeFileSync(join(dir, 'a.txt'), 'a');
  const probe = ['-c', 'echo "${HUBNESS_SECRET_KEY-unset}"'];
  process.env.HUBNESS_SECRET_KEY = 'x'.repeat(40);
  try {
    assert.equal((await gitOps.commit(dir, 'init', true)).ok, true);
    assert.equal(readFileSync(seen, 'utf8').trim(), 'unset', 'git hook');
    assert.equal((await runCli('sh', probe))?.trim(), 'unset', 'runCli');
    assert.equal((await runCli('sh', probe, 5_000, { A: '1' }))?.trim(), 'unset', 'runCli with env');
    assert.equal((await runOnce('sh', probe)).output, 'unset', 'runOnce');
    assert.equal((await sh('sh', probe)).out.trim(), 'unset', 'system sh');
  } finally {
    delete process.env.HUBNESS_SECRET_KEY;
  }
});

// ---------- csrf and preview ----------

test('API refuses non-JSON writes without the hub header (CSRF)', async () => {
  const app = new Hono();
  app.use('/api/*', csrfGuard);
  app.post('/api/x', (c) => c.json({ ok: true }));
  app.get('/api/x', (c) => c.json({ ok: true }));
  const r = async (init: RequestInit) => (await app.request('/api/x', init)).status;
  assert.equal(await r({ method: 'POST', headers: { 'content-type': 'text/plain' }, body: '{}' }), 403, 'simple cross-site POST');
  assert.equal(await r({ method: 'POST', headers: { 'content-type': 'application/x-www-form-urlencoded' }, body: 'a=1' }), 403, 'form POST');
  assert.equal(await r({ method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' }), 200);
  assert.equal(await r({ method: 'POST', headers: { 'x-hubness-request': '1' } }), 200);
  assert.equal(await r({ method: 'GET' }), 200);
});

test('preview proxy serves the app from the root with Host rewritten', async () => {
  const app = createServer((req, res) => res.end(`${req.url} ${req.headers.host}`));
  await new Promise<void>((r) => app.listen(0, '127.0.0.1', r));
  const appPort = (app.address() as { port: number }).port;
  const port = await ensurePreview('t:1', () => ({ host: '127.0.0.1', port: appPort }));
  assert.ok(port && port >= 18790 && port <= 18799);
  assert.equal(await ensurePreview('t:1', () => ({ host: '127.0.0.1', port: appPort })), port, 'same key — same port');
  const body = await fetch(`http://127.0.0.1:${port}/assets/app.js?x=1`).then((r) => r.text());
  assert.equal(body, `/assets/app.js?x=1 localhost:${appPort}`);
  closePreview('t:1');
  app.close();
});

// ---------- languages ----------

test('repository languages: bytes per language like GitHub, vendored and data files skipped', async () => {
  const { repoLanguages } = await import('../src/languages.ts');
  const dir = makeRepo(join(tmp, 'langs'));
  const { mkdirSync } = await import('node:fs');
  mkdirSync(join(dir, 'src'), { recursive: true });
  mkdirSync(join(dir, 'node_modules', 'x'), { recursive: true });
  mkdirSync(join(dir, 'docs'), { recursive: true });
  writeFileSync(join(dir, 'src', 'app.ts'), 'x'.repeat(3000));
  writeFileSync(join(dir, 'src', 'util.py'), 'x'.repeat(1000));
  writeFileSync(join(dir, 'Dockerfile'), 'FROM node');
  writeFileSync(join(dir, 'package-lock.json'), 'x'.repeat(50_000));
  writeFileSync(join(dir, 'data.json'), 'x'.repeat(50_000));
  writeFileSync(join(dir, 'docs', 'guide.ts'), 'x'.repeat(50_000));
  writeFileSync(join(dir, '.gitignore'), 'node_modules\n');
  writeFileSync(join(dir, 'node_modules', 'x', 'big.js'), 'x'.repeat(50_000));
  const langs = await repoLanguages(dir);
  assert.deepEqual(langs.map((l) => l.name), ['TypeScript', 'Python', 'Dockerfile']);
  assert.equal(langs[0]!.color, '#3178c6', 'colors come from GitHub Linguist');
  assert.ok(Math.abs(langs.reduce((a, l) => a + l.percent, 0) - 100) < 0.5);
});

// ---------- provider errors (opencode) ----------

test('opencode retry notices show the real provider reason', async () => {
  const { describeRetry } = await import('../src/agents/opencode/adapter.ts');
  const body = JSON.stringify({ error: { message: 'Provider returned error', code: 429, metadata: { raw: 'model:free is temporarily rate-limited upstream.' } } });
  const info = describeRetry({
    sessionUpdate: 'session_info_update',
    _meta: { 'opencode/retry': { attempt: 3, nextRetryAt: new Date(Date.now() + 5000).toISOString(), error: { message: 'Provider returned error', status: 429, response: { body } } } },
  });
  assert.equal(info?.error, '429 model:free is temporarily rate-limited upstream.');
  assert.match(info!.retry, /retry #3 in [45]s/);
  assert.equal(describeRetry({ sessionUpdate: 'session_info_update', _meta: {} }), null, 'other info updates are ignored');
});

test('editor save: conflict-safe, atomic, keeps mode, stays inside the project', () => {
  const root = join(tmp, 'edit-proj');
  mkdirSync(join(root, '.git'), { recursive: true });
  writeFileSync(join(root, 'run.sh'), 'echo 1\n');
  chmodSync(join(root, 'run.sh'), 0o755);
  writeFileSync(join(root, '.git', 'config'), '[core]\n');
  writeFileSync(join(root, 'blob.bin'), Buffer.from([1, 0, 2]));
  writeFileSync(join(tmp, 'outside.txt'), 'secret');
  symlinkSync(join(tmp, 'outside.txt'), join(root, 'link.txt'));
  const status = (fn: () => unknown) => {
    try {
      fn();
      return 0;
    } catch (e) {
      return (e as HttpError).status;
    }
  };

  const opened = readFileSafe(root, 'run.sh');
  assert.ok(opened.hash);
  const saved = writeFileSafe(root, 'run.sh', 'echo 2\n', opened.hash);
  assert.equal(readFileSync(join(root, 'run.sh'), 'utf8'), 'echo 2\n');
  assert.equal(statSync(join(root, 'run.sh')).mode & 0o777, 0o755, 'mode kept');
  assert.notEqual(saved.hash, opened.hash);
  assert.ok(!readdirSync(root).some((f) => f.endsWith('.tmp')), 'no temp files left');

  assert.equal(status(() => writeFileSafe(root, 'run.sh', 'echo 3\n', opened.hash)), 409, 'stale base → conflict');
  assert.equal(readFileSync(join(root, 'run.sh'), 'utf8'), 'echo 2\n', 'nothing overwritten on conflict');
  writeFileSafe(root, 'run.sh', 'echo 3\n'); // no base — explicit overwrite
  assert.equal(readFileSync(join(root, 'run.sh'), 'utf8'), 'echo 3\n');

  assert.equal(status(() => writeFileSafe(root, '../outside.txt', 'x')), 400, 'escape via ..');
  assert.equal(status(() => writeFileSafe(root, 'link.txt', 'x')), 400, 'escape via symlink');
  assert.equal(status(() => writeFileSafe(root, '.git/config', 'x')), 400, '.git is off limits');
  assert.equal(status(() => writeFileSafe(root, 'new.txt', 'x')), 404, 'only existing files');
  assert.equal(status(() => writeFileSafe(root, 'blob.bin', 'x')), 400, 'binary files');
  assert.equal(status(() => writeFileSafe(root, 'run.sh', 'a\0b')), 400, 'binary content');
  assert.equal(readFileSync(join(tmp, 'outside.txt'), 'utf8'), 'secret');
});

test('search: ripgrep and the fallback agree; case, word, regex, globs, gitignore', async () => {
  const root = join(tmp, 'search-proj');
  makeRepo(root);
  mkdirSync(join(root, 'src'), { recursive: true });
  mkdirSync(join(root, 'node_modules', 'x'), { recursive: true });
  writeFileSync(join(root, '.gitignore'), 'node_modules\n');
  writeFileSync(join(root, 'src', 'user.ts'), 'export class User {}\nconst users = new User(); // Привет User\n');
  writeFileSync(join(root, 'src', 'notes.md'), 'user guide\n');
  writeFileSync(join(root, 'node_modules', 'x', 'i.js'), 'User\n');
  const flat = (r: { files: { path: string; matches: { line: number; ranges: [number, number][] }[] }[] }) =>
    r.files.flatMap((f) => f.matches.map((m) => `${f.path}:${m.line}:${m.ranges.map((x) => x.join('-')).join(',')}`)).sort();

  for (const engine of ['rg', 'node'] as const) {
    const run = (o: Parameters<typeof searchFiles>[1]) => (engine === 'rg' ? searchFiles(root, o) : Promise.resolve(_searchNode(root, o.q, o)));
    assert.deepEqual(flat(await run({ q: 'User', caseSensitive: true })), ['src/user.ts:1:13-17', 'src/user.ts:2:18-22,36-40'], `${engine}: case + gitignore`);
    assert.deepEqual(flat(await run({ q: 'user' })).length, 3, `${engine}: case-insensitive`);
    assert.deepEqual(flat(await run({ q: 'user', word: true })), ['src/notes.md:1:0-4', 'src/user.ts:1:13-17', 'src/user.ts:2:18-22,36-40'], `${engine}: whole word`);
    assert.deepEqual(flat(await run({ q: 'Us(er)s', regex: true })), ['src/user.ts:2:6-11'], `${engine}: regex`);
    assert.deepEqual(flat(await run({ q: 'user', include: '*.md' })), ['src/notes.md:1:0-4'], `${engine}: include glob`);
    assert.deepEqual(flat(await run({ q: 'user', exclude: '*.ts' })), ['src/notes.md:1:0-4'], `${engine}: exclude glob`);
  }
  await assert.rejects(searchFiles(root, { q: '(', regex: true }), (e: HttpError) => e.status === 400);
});

test('explorer file ops: create, rename/move, delete — inside the project only', () => {
  const root = join(tmp, 'fsops-proj');
  mkdirSync(join(root, '.git'), { recursive: true });
  writeFileSync(join(tmp, 'secret.txt'), 'keep');
  symlinkSync(join(tmp, 'secret.txt'), join(root, 'link'));
  const status = (fn: () => unknown) => {
    try {
      fn();
      return 0;
    } catch (e) {
      return (e as HttpError).status;
    }
  };
  assert.equal(createEntry(root, 'src/app.py', false).path, 'src/app.py', 'parents created');
  assert.equal(readFileSync(join(root, 'src/app.py'), 'utf8'), '');
  createEntry(root, 'docs', true);
  assert.ok(statSync(join(root, 'docs')).isDirectory());
  assert.equal(status(() => createEntry(root, 'src/app.py', false)), 409, 'no overwrite');
  assert.equal(status(() => createEntry(root, '../escape.txt', false)), 400);
  assert.equal(status(() => createEntry(root, '.git/hooks/x', false)), 400);
  assert.equal(status(() => createEntry(root, '', true)), 400, 'not the root');

  assert.equal(renameEntry(root, 'src/app.py', 'src/main.py').path, 'src/main.py');
  assert.equal(renameEntry(root, 'src', 'docs/src').path, 'docs/src', 'move a folder');
  assert.ok(existsSync(join(root, 'docs/src/main.py')));
  assert.equal(status(() => renameEntry(root, 'docs', 'docs/src/inner')), 400, 'not into itself');
  createEntry(root, 'b.txt', false);
  assert.equal(status(() => renameEntry(root, 'b.txt', 'docs/src/main.py')), 409, 'no overwrite on move');
  assert.equal(status(() => renameEntry(root, 'b.txt', '../b.txt')), 400);
  // a symlink is renamed as a link; its target stays where it is
  renameEntry(root, 'link', 'link2');
  assert.ok(lstatSync(join(root, 'link2')).isSymbolicLink());
  deleteEntry(root, 'link2');
  assert.equal(readFileSync(join(tmp, 'secret.txt'), 'utf8'), 'keep', 'deleting a link keeps the target');

  deleteEntry(root, 'docs');
  assert.ok(!existsSync(join(root, 'docs')), 'folders are deleted recursively');
  assert.equal(status(() => deleteEntry(root, '.git')), 400);
  assert.equal(status(() => deleteEntry(root, 'nope')), 404);
});

test('codex: installed only when the global npm bins exist', async () => {
  const { mkdirSync, writeFileSync, symlinkSync } = await import('node:fs');
  const { codexInstalled } = await import('../src/agents/codex/adapter.ts');
  const root = join(tmp, 'codex-npm');
  const bin = join(root, 'bin');
  mkdirSync(bin, { recursive: true });
  const savedPath = process.env.PATH;
  process.env.PATH = bin;
  try {
    assert.deepEqual(codexInstalled(), { acp: null, cli: null });
    for (const [pkg, name, version] of [
      ['@zed-industries/codex-acp', 'codex-acp', '0.16.0'],
      ['@openai/codex', 'codex', '0.166.0'],
    ] as const) {
      const dir = join(root, 'lib', 'node_modules', pkg);
      mkdirSync(join(dir, 'bin'), { recursive: true });
      writeFileSync(join(dir, 'package.json'), JSON.stringify({ name: pkg, version }));
      writeFileSync(join(dir, 'bin', `${name}.js`), '');
      symlinkSync(join(dir, 'bin', `${name}.js`), join(bin, name));
    }
    assert.deepEqual(codexInstalled(), { acp: '0.16.0', cli: '0.166.0' });
  } finally {
    process.env.PATH = savedPath;
  }
});

test('claude limits: every CLI window (any plan), fresher numbers from our turns', async () => {
  const { cliCachedLimits, mergeLimits } = await import('../src/agents/claude/harness.ts');
  const at = Date.parse('2026-11-05T10:00:00Z');
  const cli = cliCachedLimits({
    cachedUsageUtilization: {
      fetchedAtMs: at,
      utilization: {
        seven_day: { utilization: 69 },
        limits: [
          { kind: 'session', group: 'session', percent: 12, resets_at: 'r1', scope: null },
          { kind: 'daily', group: 'daily', percent: 40, resets_at: 'r2', scope: null },
          { kind: 'weekly_all', group: 'weekly', percent: 69, resets_at: 'r3', scope: null },
          { kind: 'weekly_scoped', group: 'weekly', percent: 1, resets_at: 'r4', scope: { model: { display_name: 'Fable' } } },
        ],
      },
    },
  })!;
  assert.deepEqual(cli.limits.map((l) => [l.id, l.group, l.scope, l.utilization]), [
    ['session', 'session', null, 12],
    ['daily', 'daily', null, 40],
    ['weekly_all', 'weekly', null, 69],
    ['weekly_scoped:Fable', 'weekly', 'Fable', 1],
  ]);
  const ours = { value: [{ id: 'seven_day', utilization: 72, resetsAt: 'r3b' }, { id: 'five_hour', utilization: 20, resetsAt: null }], updatedAt: '2026-11-05T11:00:00.000Z' };
  const merged = mergeLimits(cli, ours);
  assert.deepEqual(merged.limits.map((l) => [l.id, l.utilization]), [['session', 20], ['daily', 40], ['weekly_all', 72], ['weekly_scoped:Fable', 1]]);
  assert.equal(merged.asOf, ours.updatedAt);
  assert.deepEqual(mergeLimits(cli, { ...ours, updatedAt: '2026-11-05T09:00:00.000Z' }).limits, cli.limits, 'older turns do not override');
});

test('claude default model: only `model` in ~/.claude/settings.json changes', async () => {
  const { mkdirSync, writeFileSync, readFileSync } = await import('node:fs');
  const { claudeDefaultModel, setClaudeDefaultModel } = await import('../src/agents/claude/harness.ts');
  const home = join(tmp, 'claude-home');
  mkdirSync(join(home, '.claude'), { recursive: true });
  const file = join(home, '.claude', 'settings.json');
  writeFileSync(file, JSON.stringify({ theme: 'dark', permissions: { allow: ['Bash(ls)'] } }));
  const saved = process.env.HOME;
  process.env.HOME = home;
  try {
    assert.equal(claudeDefaultModel(), null);
    setClaudeDefaultModel('claude-fable-5-1[1m]');
    assert.equal(claudeDefaultModel(), 'claude-fable-5-1[1m]');
    assert.deepEqual(JSON.parse(readFileSync(file, 'utf8')), { theme: 'dark', permissions: { allow: ['Bash(ls)'] }, model: 'claude-fable-5-1[1m]' });
    assert.throws(() => setClaudeDefaultModel('opus; rm -rf /'), /Bad model id/);
    setClaudeDefaultModel(null);
    assert.deepEqual(JSON.parse(readFileSync(file, 'utf8')), { theme: 'dark', permissions: { allow: ['Bash(ls)'] } });
  } finally {
    process.env.HOME = saved;
  }
});

test('host parsers: os-release, lspci GPU names, macOS vm_stat / swapusage / launchctl', async () => {
  const { parseOsRelease, gpuName } = await import('../src/system/host.ts');
  const { parseVmStat, parseSwapUsage, parseLaunchctlList } = await import('../src/system/monitor.ts');
  const os = parseOsRelease('NAME="Linux Mint"\nID=linuxmint\nID_LIKE="ubuntu debian"\nPRETTY_NAME="Linux Mint 22"\n');
  assert.equal(os.ID, 'linuxmint');
  assert.equal(os.ID_LIKE, 'ubuntu debian');
  assert.equal(gpuName('Advanced Micro Devices, Inc. [AMD/ATI]', 'Strix [Radeon 880M / 890M]'), 'AMD Radeon 880M / 890M');
  assert.equal(gpuName('NVIDIA Corporation', 'AD104 [GeForce RTX 4070]'), 'NVIDIA GeForce RTX 4070');
  const vm = 'Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:                               1000.\nPages active:                             5000.\nPages inactive:                           2000.\nPages speculative:                         500.\nPages purgeable:                           100.\n';
  assert.equal(Math.round(parseVmStat(vm)), Math.round((3600 * 16384) / 1048576));
  assert.deepEqual(parseSwapUsage('total = 2048.00M  used = 1024.50M  free = 1023.50M  (encrypted)'), { totalMb: 2048, usedMb: 1025 });
  assert.deepEqual(parseSwapUsage('total = 1.50G  used = 0.00M  free = 1.50G'), { totalMb: 1536, usedMb: 0 });
  const units = parseLaunchctlList('PID\tStatus\tLabel\n412\t0\tcom.example.web\n-\t78\tcom.example.broken\n-\t0\tcom.apple.Safari\n');
  assert.deepEqual(
    units.map((u) => [u.name, u.active]),
    [
      ['com.example.web', 'active'],
      ['com.example.broken', 'failed'],
      ['com.apple.Safari', 'inactive'],
    ],
  );
});

test('terminal jobs answer Primary Device Attributes themselves (fish waits for it at start)', async () => {
  const { startJob, getJob, killJob } = await import('../src/terminal.ts');
  const job = startJob({ title: 'da1', command: `printf '\\033[0c'; IFS= read -r -s -d c -t 5 r; printf 'REPLY:%s:END' "\${r#?}"` });
  for (let i = 0; i < 50 && !/END/.test(getJob(job.id)?.output ?? ''); i++) await new Promise((r) => setTimeout(r, 100));
  const out = getJob(job.id)?.output ?? '';
  killJob(job.id);
  assert.match(out, /REPLY:\[\?1;2:END/);
});

test('default instructions from Settings → Agent go into every new normal project', async () => {
  const { setDefaultInstructions, settings } = await import('../src/catalog.ts');
  const before = projects.create('before-defaults', join(tmp, 'p-before'));
  setDefaultInstructions('  Answer in Russian.  ');
  assert.equal(settings().defaultInstructions, 'Answer in Russian.');
  const fresh = projects.create('with-defaults', join(tmp, 'p-with'));
  assert.equal(fresh.instructions, 'Answer in Russian.');
  assert.equal(projects.get(before.id)!.instructions, '', 'existing projects keep theirs');
  assert.equal(projects.create('sys', join(tmp, 'p-sys'), 'system').instructions, '', 'not for the system project');
  setDefaultInstructions('');
  assert.equal(projects.create('after-clear', join(tmp, 'p-after')).instructions, '');
});
