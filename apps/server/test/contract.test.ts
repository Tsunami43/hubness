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

type Fetcher = (url: string, init?: RequestInit) => Promise<Response>;
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

async function makeClient(): Promise<{ call: Call; raw: Fetcher }> {
  const send = async (fetcher: Fetcher, method: string, url: string, body?: unknown, headers?: Record<string, string>) => {
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
    const raw: Fetcher = (u, init) => fetch(base + u, init);
    return { raw, call: (method, url, body, headers) => send(raw, method, url, body, headers) };
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
  const raw: Fetcher = async (u, init) => app.request(u, init);
  return { raw, call: (method, url, body, headers) => send(raw, method, url, body, headers) };
}

const { call, raw } = await makeClient();

/** Open an SSE stream; next() resolves with the next `data:` message parsed as JSON. */
async function openStream(path: string) {
  const res = await raw(`/api${path}`);
  assert.equal(res.status, 200);
  assert.match(res.headers.get('content-type') ?? '', /text\/event-stream/);
  const reader = res.body!.getReader();
  const decoder = new TextDecoder();
  let buf = '';
  const next = async (): Promise<any> => {
    for (;;) {
      const end = buf.indexOf('\n\n');
      if (end >= 0) {
        const chunk = buf.slice(0, end);
        buf = buf.slice(end + 2);
        const data = chunk
          .split('\n')
          .filter((l) => l.startsWith('data:'))
          .map((l) => l.slice(5).trimStart())
          .join('\n');
        if (data) return JSON.parse(data);
        continue;
      }
      const { value, done } = await reader.read();
      if (done) throw new Error('stream ended');
      buf += decoder.decode(value, { stream: true });
    }
  };
  // the TS server subscribes asynchronously after answering: give it a moment
  await new Promise((r) => setTimeout(r, 50));
  return { next, close: () => reader.cancel().catch(() => {}) };
}
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

test('sessions: scopes, patch, archive, pane and live streams', async () => {
  const defaultMode = (await call('GET', '/settings')).json.defaultPermission;
  const repo = makeRepo(join(tmp, 'elsewhere', 'gamma'));
  const [project] = (await call('POST', '/projects/import', { paths: [repo] })).json;
  const group = (await call('POST', '/groups', { name: 'Sessions', projectIds: [project.id] })).json;
  const global = await openStream('/stream');

  assert.equal((await call('POST', '/sessions', {})).status, 400, 'agent is required');
  assert.equal((await call('POST', '/sessions', { agent: { kind: 'nope' } })).status, 400);
  assert.equal((await call('POST', '/sessions', { agent: { kind: 'codex' }, title: 'x'.repeat(501) })).status, 400);
  assert.equal((await call('POST', '/sessions', { projectId: project.id, groupId: group.id, agent: { kind: 'codex' } })).status, 400);
  assert.equal((await call('POST', '/sessions', { projectId: 'missing', agent: { kind: 'codex' } })).status, 404);

  const chat = await call('POST', '/sessions', { agent: { kind: 'codex', model: '  ' } });
  assert.equal(chat.status, 201);
  assert.equal(chat.json.title, 'New chat');
  assert.equal(chat.json.status, 'idle');
  assert.equal(chat.json.projectId, null);
  assert.equal(chat.json.workspaceId, null);
  assert.equal(chat.json.archivedAt, null);
  assert.deepEqual(chat.json.agent, { kind: 'codex', permissionMode: defaultMode }, 'empty model dropped, default mode applied');
  assert.ok(chat.json.cwd.startsWith(join(tmp, 'hub', 'inbox') + '/'));
  assert.ok(existsSync(chat.json.cwd), 'the chat gets its own Inbox folder');
  const announced = await global.next();
  assert.equal(announced.type, 'session');
  assert.equal(announced.session.id, chat.json.id);

  const inProject = await call('POST', '/sessions', { projectId: project.id, title: '  Fix it  ', agent: { kind: 'claude-code', permissionMode: 'plan' } });
  assert.equal(inProject.status, 201);
  assert.equal(inProject.json.title, 'Fix it');
  assert.equal(inProject.json.cwd, null);
  assert.equal(inProject.json.agent.permissionMode, 'plan');
  assert.equal(typeof inProject.json.workspaceId, 'string');
  const inGroup = (await call('POST', '/sessions', { groupId: group.id, agent: { kind: 'opencode' } })).json;
  assert.equal(inGroup.title, 'New session');
  assert.equal(inGroup.groupId, group.id);

  const ids = async (q: string) => (await call('GET', `/sessions${q}`)).json.map((x: any) => x.id);
  assert.deepEqual(await ids(''), [chat.json.id], 'plain chats only by default');
  assert.deepEqual(await ids(`?projectId=${project.id}`), [inProject.json.id]);
  assert.deepEqual(await ids(`?groupId=${group.id}`), [inGroup.id]);
  assert.deepEqual(new Set(await ids('?all=1')), new Set([chat.json.id, inProject.json.id, inGroup.id]));
  assert.equal((await call('GET', '/sessions/missing')).status, 404);
  assert.equal((await call('GET', `/sessions/${chat.json.id}`)).json.id, chat.json.id);

  const live = await openStream(`/sessions/${chat.json.id}/stream`);
  const snap = await live.next();
  assert.equal(snap.type, 'snapshot');
  assert.equal(snap.session.id, chat.json.id);
  assert.deepEqual(snap.events, []);
  assert.equal(snap.pane, null);
  assert.equal((await raw('/api/sessions/missing/stream')).status, 404);

  const renamed = await call('PATCH', `/sessions/${chat.json.id}`, { title: '  Renamed  ', agent: { kind: 'grok', model: ' gpt-5 ' } });
  assert.equal(renamed.status, 200);
  assert.equal(renamed.json.title, 'Renamed');
  assert.deepEqual(renamed.json.agent, { kind: 'codex', model: 'gpt-5', permissionMode: defaultMode }, 'the agent kind never changes');
  assert.ok(renamed.json.updatedAt >= chat.json.updatedAt);
  assert.equal((await call('PATCH', `/sessions/${chat.json.id}`, { title: '' })).json.title, 'Renamed', 'an empty title is ignored');
  const update = await live.next();
  assert.equal(update.type, 'session');
  assert.equal(update.session.title, 'Renamed');
  let g = await global.next();
  while (g.session?.id !== chat.json.id) g = await global.next();
  assert.equal(g.session.title, 'Renamed');

  assert.equal((await call('POST', `/sessions/${chat.json.id}/pane`, {})).status, 400);
  assert.equal((await call('POST', `/sessions/${chat.json.id}/pane`, { pane: { kind: 'video', target: 'x' } })).status, 400);
  assert.deepEqual((await call('POST', `/sessions/${chat.json.id}/pane`, { pane: { kind: 'url', target: 'http://localhost:5173', title: 'App' } })).json, { ok: true });
  let paneMsg = await live.next();
  while (paneMsg.type !== 'pane') paneMsg = await live.next();
  assert.equal(paneMsg.pane.kind, 'url');
  assert.equal(paneMsg.pane.target, 'http://localhost:5173');
  assert.equal(paneMsg.pane.title, 'App');
  live.close();
  const again = await openStream(`/sessions/${chat.json.id}/stream`);
  assert.equal((await again.next()).pane.target, 'http://localhost:5173', 'the pane is in the snapshot');
  again.close();

  assert.deepEqual((await call('GET', `/sessions/${chat.json.id}/events`)).json, []);
  assert.deepEqual((await call('POST', `/sessions/${chat.json.id}/stop`)).json, { ok: false }, 'nothing is running');

  const before = (await call('GET', `/sessions/${chat.json.id}`)).json;
  const archived = await call('POST', `/sessions/${chat.json.id}/archive`);
  assert.equal(archived.status, 200);
  assert.equal(typeof archived.json.archivedAt, 'string');
  assert.equal(archived.json.updatedAt, before.updatedAt, 'archiving does not bump updatedAt');
  assert.deepEqual(await ids(''), [], 'archived chats leave the list');
  assert.deepEqual(await ids('?archived=1'), [chat.json.id]);
  const restored = await call('POST', `/sessions/${chat.json.id}/restore`);
  assert.equal(restored.json.archivedAt, null);
  assert.equal((await call('POST', '/sessions/missing/archive')).status, 404);

  await call('POST', `/sessions/${inGroup.id}/archive`);
  const emptied = await call('DELETE', '/sessions/archive');
  assert.deepEqual(emptied.json, { ok: true, deleted: 1 });
  let gone = await global.next();
  while (gone.type !== 'session-deleted') gone = await global.next();
  assert.equal(gone.id, inGroup.id);

  assert.deepEqual((await call('DELETE', `/sessions/${chat.json.id}`)).json, { ok: true });
  assert.equal((await call('GET', `/sessions/${chat.json.id}`)).status, 404);
  global.close();
});
