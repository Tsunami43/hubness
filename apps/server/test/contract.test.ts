import { makeRepo, tmp } from './setup.ts';
import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { spawn, type ChildProcess } from 'node:child_process';
import { existsSync, lstatSync, readlinkSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

/**
 * HTTP contract tests: black-box checks of the API that every server implementation must
 * pass. By default they run against the TypeScript server in-process; with
 * HUBNESS_CONTRACT_SERVER=rust they start the Rust binary (`cargo build` first; the path
 * can be overridden with HUBNESS_RUST_BIN) on a free port with the same isolated folders.
 *
 *   pnpm test:contract:rust
 */

type Call = (method: string, url: string, body?: unknown, headers?: Record<string, string>) => Promise<{ status: number; json: any }>;

const target = process.env.HUBNESS_CONTRACT_SERVER ?? 'ts';
const repoRoot = resolve(import.meta.dirname, '../../..');
let child: ChildProcess | null = null;

async function startRust(): Promise<string> {
  const bin = process.env.HUBNESS_RUST_BIN ?? join(repoRoot, 'target/debug/hubness');
  if (!existsSync(bin)) throw new Error(`Rust server binary not found: ${bin} (run cargo build)`);
  child = spawn(bin, ['serve'], {
    cwd: tmp, // no .env from the repository
    env: { ...process.env, HUBNESS_PORT: '0', HUBNESS_WEB_DIST: join(tmp, 'no-web') },
    stdio: ['ignore', 'pipe', 'inherit'],
  });
  return new Promise((ok, fail) => {
    let out = '';
    child!.stdout!.on('data', (d: Buffer) => {
      out += d.toString();
      const m = out.match(/hubness: (http:\/\/\S+)/);
      if (m) ok(m[1]);
    });
    child!.on('exit', (code) => fail(new Error(`Rust server exited with ${code}: ${out}`)));
  });
}

async function makeClient(): Promise<Call> {
  const send = async (fetcher: (url: string, init: RequestInit) => Promise<Response>, method: string, url: string, body?: unknown, headers?: Record<string, string>) => {
    const res = await fetcher(`/api${url}`, {
      method,
      // like the web client: JSON bodies, or the CSRF header on bodyless mutations
      headers: { ...(body !== undefined ? { 'Content-Type': 'application/json' } : { 'X-Hubness-Request': '1' }), ...headers },
      body: body === undefined ? undefined : typeof body === 'string' ? body : JSON.stringify(body),
    });
    const text = await res.text();
    let json: any = null;
    try {
      json = JSON.parse(text);
    } catch {
      json = text;
    }
    return { status: res.status, json };
  };
  if (target === 'rust') {
    const base = await startRust();
    return (method, url, body, headers) => send((u, init) => fetch(base + u, init), method, url, body, headers);
  }
  // the TypeScript server, assembled as in src/index.ts
  const { Hono } = await import('hono');
  const { api } = await import('../src/routes/api.ts');
  const { csrfGuard } = await import('../src/csrf.ts');
  const { HttpError } = await import('../src/errors.ts');
  const { ensureHomeProject } = await import('../src/system/ask.ts');
  ensureHomeProject();
  const app = new Hono();
  app.onError((err, c) => c.json({ error: err.message }, err instanceof HttpError ? (err.status as 400) : 500));
  app.use('/api/*', csrfGuard);
  app.route('/api', api);
  app.all('/api/*', (c) => c.json({ error: 'Not found' }, 404));
  return (method, url, body, headers) => send(async (u, init) => app.request(u, init), method, url, body, headers);
}

const call = await makeClient();
after(() => child?.kill('SIGTERM'));

const projectsRoot = join(tmp, 'hub', 'projects');
makeRepo(join(projectsRoot, 'alpha'));
makeRepo(join(projectsRoot, 'team', 'beta'));
makeRepo(join(projectsRoot, 'node_modules', 'skipped'));

test('health, unknown routes and CSRF', async () => {
  assert.deepEqual((await call('GET', '/health')).json, { ok: true });
  const missing = await call('GET', '/definitely-not-a-route');
  assert.equal(missing.status, 404);
  assert.equal(typeof missing.json.error, 'string');
  const plain = await call('PUT', '/settings', 'x', { 'Content-Type': 'text/plain' });
  assert.equal(plain.status, 403, 'a non-JSON mutation without the header is refused');
  const broken = await call('PUT', '/settings', '{oops');
  assert.equal(broken.status, 400);
});

test('the home system project exists', async () => {
  const list = (await call('GET', '/projects')).json;
  const home = list.find((p: any) => p.kind === 'system');
  assert.ok(home, 'system project is created at startup');
  assert.equal(home.name, 'home');
  assert.ok(existsSync(join(home.path, 'AGENTS.md')));
  const del = await call('DELETE', `/projects/${home.id}`);
  assert.equal(del.status, 400, 'the system project cannot be removed');
});

test('settings', async () => {
  const s = (await call('GET', '/settings')).json;
  assert.equal(s.projectsRoot, projectsRoot);
  assert.equal(s.defaultPermission, 'ask');
  assert.equal((await call('PUT', '/settings', {})).status, 400, 'nothing to save');
  assert.equal((await call('PUT', '/settings', { defaultPermission: 'yolo' })).status, 400);
  assert.equal((await call('PUT', '/settings', { defaultInstructions: 'x'.repeat(20_001) })).status, 400);
  const saved = await call('PUT', '/settings', { defaultPermission: 'acceptEdits', defaultInstructions: '  Be brief.  ' });
  assert.equal(saved.status, 200);
  assert.equal(saved.json.defaultPermission, 'acceptEdits');
  assert.equal(saved.json.defaultInstructions, 'Be brief.');
  assert.equal(saved.json.sessionInstructions, '');
});

test('discover → import → patch → remove a project', async () => {
  const found = (await call('GET', '/projects/discover')).json;
  assert.equal(found.root, projectsRoot);
  assert.deepEqual(
    found.repos.map((r: any) => r.name),
    ['alpha', 'beta'],
    'nested repositories are found, node_modules is skipped',
  );
  assert.equal((await call('GET', `/projects/discover?path=${encodeURIComponent(join(tmp, 'nope'))}`)).status, 400);

  assert.equal((await call('POST', '/projects/import', { paths: [] })).status, 400);
  assert.equal((await call('POST', '/projects/import', {})).status, 400);
  assert.equal((await call('POST', '/projects/import', { paths: [join(tmp, 'nope')] })).status, 400);
  const notRepo = join(tmp, 'hub');
  assert.match((await call('POST', '/projects/import', { paths: [notRepo] })).json.error, /Not a git repository/);
  assert.equal((await call('POST', '/projects/import', { paths: [projectsRoot + '/alpha'], groupId: 'missing' })).status, 404);

  const imported = await call('POST', '/projects/import', { paths: found.repos.map((r: any) => r.path + '/./') });
  assert.equal(imported.status, 201);
  assert.equal(imported.json.length, 2);
  const [alpha] = imported.json;
  assert.equal(alpha.path, join(projectsRoot, 'alpha'), 'paths are normalized');
  assert.equal(alpha.kind, 'normal');
  assert.equal(alpha.instructions, 'Be brief.', 'a new project starts with the default instructions');
  assert.deepEqual(alpha.groupIds, []);
  assert.equal(alpha.gitProvider, null);
  assert.match(alpha.createdAt, /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/);
  const again = await call('POST', '/projects/import', { paths: [alpha.path] });
  assert.equal(again.json[0].id, alpha.id, 'importing twice does not duplicate');
  assert.equal((await call('GET', '/projects/discover')).json.repos.length, 0, 'imported repos are no longer suggested');

  assert.equal((await call('PATCH', `/projects/${alpha.id}`, { name: '   ' })).status, 400);
  assert.equal((await call('PATCH', `/projects/${alpha.id}`, { name: 'x'.repeat(121) })).status, 400);
  assert.equal((await call('PATCH', `/projects/${alpha.id}`, { name: 5 })).status, 400);
  assert.equal((await call('PATCH', '/projects/missing', { name: 'x' })).status, 404);
  const patched = await call('PATCH', `/projects/${alpha.id}`, { name: '  Alpha  ', description: 'First' });
  assert.equal(patched.status, 200);
  assert.equal(patched.json.name, 'Alpha');
  assert.equal(patched.json.description, 'First');
  assert.equal(patched.json.instructions, 'Be brief.', 'fields not sent are kept');

  const gone = makeRepo(join(tmp, 'elsewhere', 'gone'));
  const g = (await call('POST', '/projects/import', { paths: [gone] })).json[0];
  const { rmSync } = await import('node:fs');
  rmSync(gone, { recursive: true });
  const listed = (await call('GET', '/projects')).json.find((p: any) => p.id === g.id);
  assert.equal(listed.missing, true);
  assert.equal((await call('PATCH', `/projects/${g.id}`, { name: 'x' })).status, 410);
  const removed = await call('DELETE', `/projects/${g.id}`);
  assert.equal(removed.status, 200);
  assert.equal(removed.json.ok, true);
  assert.equal(removed.json.deleted, null);
  assert.equal((await call('DELETE', `/projects/${g.id}`)).status, 404);
  assert.ok(!(await call('GET', '/projects')).json.some((p: any) => p.id === g.id));
});

test('groups with a symlink workspace', async () => {
  const projects = (await call('GET', '/projects')).json.filter((p: any) => p.kind === 'normal');
  const ids = projects.map((p: any) => p.id);
  const home = (await call('GET', '/projects')).json.find((p: any) => p.kind === 'system');

  assert.equal((await call('POST', '/groups', { projectIds: ids })).status, 400, 'name is required');
  assert.equal((await call('POST', '/groups', { name: 'G', projectIds: ['missing'] })).status, 400);
  assert.equal((await call('POST', '/groups', { name: 'G', projectIds: [home.id] })).status, 400, 'only normal projects');
  assert.equal((await call('POST', '/groups', { name: 'G', folder: 'relative/dir' })).status, 400);
  const file = join(tmp, 'a-file');
  writeFileSync(file, '');
  assert.equal((await call('POST', '/groups', { name: 'G', folder: file })).status, 400);

  const created = await call('POST', '/groups', { name: ' Platform ', projectIds: [...ids, ids[0]], folder: join(tmp, 'store') });
  assert.equal(created.status, 201);
  const g = created.json;
  assert.equal(g.name, 'Platform');
  assert.deepEqual(g.projectIds, ids, 'duplicates are dropped, order kept');
  assert.equal(g.projects.length, ids.length);
  assert.equal(g.folder, join(tmp, 'store'));
  assert.equal(g.workspace.scope, 'group');
  assert.equal(g.workspace.strategy, 'multi');
  assert.equal(g.workspace.status, 'ready');
  assert.deepEqual(g.workspace.capabilities, { lifecycle: false, clones: false });
  for (const p of projects) {
    const link = join(g.workspace.path, p.path.split('/').pop());
    assert.ok(lstatSync(link).isSymbolicLink());
    assert.equal(readlinkSync(link), p.path);
  }

  const memberOf = (await call('GET', '/projects')).json.find((p: any) => p.id === ids[0]);
  assert.deepEqual(memberOf.groupIds, [g.id]);
  assert.deepEqual((await call('GET', '/groups')).json.map((x: any) => x.id), [g.id]);
  assert.equal((await call('GET', '/groups/missing')).status, 404);

  const patched = await call('PATCH', `/groups/${g.id}`, { projectIds: [ids[1]], folder: null, instructions: 'Shared' });
  assert.equal(patched.status, 200);
  assert.deepEqual(patched.json.projectIds, [ids[1]]);
  assert.equal(patched.json.folder, null);
  assert.equal(patched.json.name, 'Platform', 'fields not sent are kept');
  assert.equal(patched.json.instructions, 'Shared');
  const stale = join(g.workspace.path, projects[0].path.split('/').pop());
  assert.ok(!existsSync(stale), 'links of removed members are dropped');
  assert.equal((await call('PATCH', `/groups/${g.id}`, { name: '' })).status, 400);

  const imported = await call('POST', '/projects/import', { paths: [projects[0].path], groupId: g.id });
  assert.equal(imported.status, 201);
  assert.deepEqual((await call('GET', `/groups/${g.id}`)).json.projectIds, [ids[1], ids[0]], 'import appends to the group');

  const del = await call('DELETE', `/groups/${g.id}`);
  assert.deepEqual(del.json, { ok: true });
  assert.ok(!existsSync(g.workspace.path), 'the group workspace folder is removed');
  assert.ok(existsSync(projects[0].path), 'member repositories are untouched');
  assert.equal((await call('DELETE', `/groups/${g.id}`)).status, 404);
});
