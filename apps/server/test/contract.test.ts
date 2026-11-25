import { makeRepo, tmp } from './setup.ts';
import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync, spawn, type ChildProcess } from 'node:child_process';
import { chmodSync, existsSync, lstatSync, readlinkSync, writeFileSync } from 'node:fs';
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
let rustBase = '';

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
    const base = (rustBase = await startRust());
    const raw: Fetcher = (u, init) => fetch(base + u, init);
    return { raw, call: (method, url, body, headers) => send(raw, method, url, body, headers) };
  }
  // the TypeScript server, assembled as in src/index.ts
  const { Hono } = await import('hono');
  const { api } = await import('../src/routes/api.ts');
  const { csrfGuard } = await import('../src/csrf.ts');
  const { HttpError } = await import('../src/errors.ts');
  const { ensureHomeProject } = await import('../src/system/ask.ts');
  const { harness, jobs } = await import('../src/routes/harness.ts');
  ensureHomeProject();
  const app = new Hono();
  app.onError((err, c) => c.json({ error: err.message }, err instanceof HttpError ? (err.status as 400) : 500));
  app.use('/api/*', csrfGuard);
  app.route('/api/harness', harness);
  app.route('/api/jobs', jobs);
  app.route('/api', api);
  app.all('/api/*', (c) => c.json({ error: 'Not found' }, 404));
  const raw: Fetcher = async (u, init) => app.request(u, init);
  return { raw, call: (method, url, body, headers) => send(raw, method, url, body, headers) };
}

// a fake Claude Code CLI: replays the fixture named in <tmp>/claude-fixture (see fixtures/replay-agent.ts)
const fixtureFile = join(tmp, 'claude-fixture');
const fakeClaude = join(tmp, 'fake-claude');
const replayAgent = join(import.meta.dirname, 'fixtures', 'replay-agent.ts');
writeFileSync(
  fakeClaude,
  `#!/bin/sh\nREPLAY_MODE=stream REPLAY_FIXTURE="$(cat ${fixtureFile})" exec ${process.execPath} --disable-warning=ExperimentalWarning ${replayAgent}\n`,
);
chmodSync(fakeClaude, 0o755);
process.env.HUBNESS_CLAUDE_BIN = fakeClaude;
// ACP agents (Codex adapter, OpenCode) replay the same fixture file in ACP mode
const fakeAcp = join(tmp, 'fake-acp');
writeFileSync(
  fakeAcp,
  `#!/bin/sh\nREPLAY_MODE=acp REPLAY_FIXTURE="$(cat ${fixtureFile})" exec ${process.execPath} --disable-warning=ExperimentalWarning ${replayAgent}\n`,
);
chmodSync(fakeAcp, 0o755);
process.env.HUBNESS_CODEX_ACP = fakeAcp;
process.env.HUBNESS_OPENCODE_BIN = fakeAcp;
const useFixture = (name: string) => writeFileSync(fixtureFile, join(import.meta.dirname, 'fixtures', 'agents', name));

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
after(() => {
  child?.kill('SIGTERM');
  // a failed test may leave a fake agent or a stream behind: don't hang (keeps the exit code)
  setTimeout(() => process.exit(), 500).unref();
});

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

/** Poll the session until `pred` holds (agents run asynchronously). */
async function waitFor(id: string, pred: (s: any) => boolean, what: string) {
  for (let i = 0; i < 500; i++) {
    const s = (await call('GET', `/sessions/${id}`)).json;
    if (pred(s)) return s;
    await new Promise((r) => setTimeout(r, 20));
  }
  const events = (await call('GET', `/sessions/${id}/events`)).json;
  throw new Error(`timed out waiting for ${what}: ${JSON.stringify(events.slice(-3))}`);
}

/** The feed without volatile fields (uids carry timestamps; seq order is kept). */
const feed = async (id: string) =>
  (await call('GET', `/sessions/${id}/events`)).json.map(({ uid, seq: _s, ts: _t, durationMs: _d, ...rest }: any) => ({ uid: uid.split(':')[0], ...rest }));

test('claude-code turn: streaming, tools, limits, result and resume id', async () => {
  useFixture('claude-tool-turn.jsonl');
  const s = (await call('POST', '/sessions', { agent: { kind: 'claude-code' } })).json;
  assert.equal((await call('POST', `/sessions/${s.id}/messages`, { text: '   ' })).status, 400, 'empty message');
  const live = await openStream(`/sessions/${s.id}/stream`);
  await live.next(); // snapshot
  const started = await call('POST', `/sessions/${s.id}/messages`, { text: 'What is in   this folder?' });
  assert.equal(started.status, 200);
  assert.equal(started.json.title, 'What is in this folder?', 'the first message names the chat');
  const done = await waitFor(s.id, (x) => x.status === 'idle' && x.agentSessionId, 'the turn to finish');
  assert.equal(done.agentSessionId, 'sess-claude-1');
  assert.deepEqual(await feed(s.id), [
    { uid: 'u', kind: 'user', text: 'What is in   this folder?' },
    { uid: 'th', kind: 'thinking', text: 'List the folder first.' },
    { uid: 't', kind: 'text', text: 'Checking the files.', streaming: false },
    { uid: 'tool', kind: 'tool', toolId: 'toolu_1', name: 'Bash', input: { command: 'ls', description: 'List files' }, status: 'ok', output: 'README.md\nsrc' },
    { uid: 'tool', kind: 'tool', toolId: 'toolu_2', name: 'Read', input: { file_path: '/work/missing.txt' }, status: 'error', output: 'File does not exist.' },
    { uid: 't', kind: 'text', text: 'There is a README and a src folder.' },
    { uid: 'res', kind: 'result', ok: true, billing: 'subscription', costUsd: 0.164, limit: { window: 'seven_day', utilization: 0.73 }, model: 'claude-opus-5-5' },
  ]);
  // the stream saw the text grow before it was persisted
  const seen: any[] = [];
  for (;;) {
    const m = await live.next();
    seen.push(m);
    if (m.type === 'session' && m.session.status === 'idle') break;
  }
  live.close();
  assert.ok(seen.some((m) => m.type === 'event' && m.event.streaming === true), 'streaming deltas are broadcast');
  assert.ok(seen.some((m) => m.type === 'session' && m.session.status === 'running'));

  useFixture('claude-error.jsonl');
  await call('POST', `/sessions/${s.id}/messages`, { text: 'again' });
  const failed = await waitFor(s.id, (x) => x.status !== 'running', 'the failed turn');
  assert.equal(failed.status, 'idle', 'the turn ended with a result, not a crash');
  const last = (await feed(s.id)).slice(-3);
  assert.deepEqual(last.map((e: any) => e.kind), ['user', 'notice', 'result']);
  assert.equal(last[1].text, 'API Error: 529 overloaded');
  assert.equal(last[2].ok, false);
  assert.equal(last[2].billing, 'api');
});

test('claude-code approvals and stop', async () => {
  useFixture('claude-approval.jsonl');
  {
    // (the fake agent lives in /tmp, which the bubblewrap sandbox of project sessions hides: a chat)
    const s = (await call('POST', '/sessions', { agent: { kind: 'claude-code' }, prompt: 'clean the build' })).json;
    assert.equal(s.title, 'clean the build', 'a prompt on create starts the first turn');
    await waitFor(s.id, (x) => x.status === 'waiting', 'the permission request');
    const ap = (await feed(s.id)).find((e: any) => e.kind === 'approval');
    assert.deepEqual(ap, { uid: 'ap', kind: 'approval', requestId: 'req-1', toolName: 'Bash', input: { command: 'rm -rf build' }, status: 'pending' });
    assert.equal((await call('POST', `/sessions/${s.id}/messages`, { text: 'more' })).status, 409, 'one turn at a time');
    assert.equal((await call('POST', `/sessions/${s.id}/archive`)).status, 409, 'a working session is not archived');
    assert.equal((await call('POST', `/sessions/${s.id}/approvals/req-1`, { decision: 'maybe' })).status, 400);
    assert.equal((await call('POST', `/sessions/${s.id}/approvals/nope`, { decision: 'allow' })).status, 404);
    assert.deepEqual((await call('POST', `/sessions/${s.id}/approvals/req-1`, { decision: 'allow' })).json, { ok: true });
    const done = await waitFor(s.id, (x) => x.status === 'idle', 'the turn after approval');
    assert.equal(done.agentSessionId, 'sess-claude-2');
    const events = await feed(s.id);
    assert.equal(events.find((e: any) => e.kind === 'approval').status, 'allowed');
    assert.equal(events.find((e: any) => e.kind === 'tool').status, 'ok');
    assert.equal(events.at(-1).costUsd, 0.24259540000000002, 'floats survive exactly');
  }

  const s = (await call('POST', '/sessions', { agent: { kind: 'claude-code' }, prompt: 'clean the build' })).json;
  await waitFor(s.id, (x) => x.status === 'waiting', 'the permission request');
  assert.deepEqual((await call('POST', `/sessions/${s.id}/stop`)).json, { ok: true });
  await waitFor(s.id, (x) => x.status === 'idle', 'the stopped turn');
  const events = await feed(s.id);
  assert.equal(events.find((e: any) => e.kind === 'approval').status, 'expired');
  assert.deepEqual(events.at(-1), { uid: 'stop', kind: 'notice', level: 'info', text: 'Stopped' });
  assert.deepEqual((await call('POST', `/sessions/${s.id}/stop`)).json, { ok: false }, 'nothing left to stop');
});

test('project secrets: names only, validation, removal', async () => {
  const repo = makeRepo(join(tmp, 'elsewhere', 'secrets'));
  const [p] = (await call('POST', '/projects/import', { paths: [repo] })).json;
  const empty = (await call('GET', `/projects/${p.id}/secrets`)).json;
  assert.deepEqual(empty, { secrets: [], unreadable: [], keySource: 'file' });
  assert.equal((await call('PUT', `/projects/${p.id}/secrets/1BAD`, { value: 'x' })).status, 400);
  assert.equal((await call('PUT', `/projects/${p.id}/secrets/PATH`, { value: 'x' })).status, 400, 'reserved');
  assert.equal((await call('PUT', `/projects/${p.id}/secrets/HUBNESS_X`, { value: 'x' })).status, 400, 'reserved prefix');
  assert.equal((await call('PUT', `/projects/${p.id}/secrets/API_TOKEN`, { value: '' })).status, 400);
  assert.equal((await call('PUT', `/projects/missing/secrets/API_TOKEN`, { value: 'x' })).status, 404);
  const saved = await call('PUT', `/projects/${p.id}/secrets/API_TOKEN`, { value: 's3cr3t' });
  assert.equal(saved.status, 200);
  assert.deepEqual(saved.json.map((x: any) => x.name), ['API_TOKEN']);
  assert.ok(!JSON.stringify(saved.json).includes('s3cr3t'), 'values never reach the client');
  await call('PUT', `/projects/${p.id}/secrets/ANOTHER`, { value: 'v' });
  const listed = (await call('GET', `/projects/${p.id}/secrets`)).json;
  assert.deepEqual(listed.secrets.map((x: any) => x.name), ['ANOTHER', 'API_TOKEN']);
  assert.deepEqual(listed.unreadable, []);
  assert.deepEqual((await call('DELETE', `/projects/${p.id}/secrets/ANOTHER`)).json.map((x: any) => x.name), ['API_TOKEN']);
});

test('kanban cards and starting an agent from a card', async () => {
  const repo = makeRepo(join(tmp, 'elsewhere', 'board'));
  const [p] = (await call('POST', '/projects/import', { paths: [repo] })).json;
  assert.equal((await call('POST', `/projects/${p.id}/cards`, { title: '   ' })).status, 400);
  assert.equal((await call('POST', `/projects/${p.id}/cards`, { title: 'x', column: 'later' })).status, 400);
  assert.equal((await call('POST', '/projects/missing/cards', { title: 'x' })).status, 404);
  const a = await call('POST', `/projects/${p.id}/cards`, { title: ' First ' });
  assert.equal(a.status, 201);
  assert.equal(a.json.title, 'First');
  assert.equal(a.json.column, 'todo');
  assert.equal(a.json.position, 1);
  assert.equal(a.json.description, '');
  assert.equal(a.json.sessionId, null);
  const b = (await call('POST', `/projects/${p.id}/cards`, { title: 'Second', description: 'Details', column: 'todo' })).json;
  assert.equal(b.position, 2);
  const moved = await call('PATCH', `/cards/${a.json.id}`, { column: 'done' });
  assert.equal(moved.json.column, 'done');
  assert.equal(moved.json.position, 1, 'last in the new column');
  assert.equal((await call('PATCH', `/cards/${a.json.id}`, { position: 7.5, title: '' })).json.position, 7.5);
  assert.equal((await call('PATCH', '/cards/missing', { title: 'x' })).status, 404);
  assert.deepEqual((await call('GET', `/projects/${p.id}/cards`)).json.map((c: any) => c.id), [a.json.id, b.id], 'ordered by column, position');

  useFixture('claude-tool-turn.jsonl');
  assert.equal((await call('POST', `/cards/${b.id}/start`, { agent: { kind: 'nope' } })).status, 400);
  const started = await call('POST', `/cards/${b.id}/start`, { agent: { kind: 'claude-code' } });
  assert.equal(started.status, 201);
  assert.equal(started.json.cardId, b.id);
  assert.equal(started.json.projectId, p.id);
  assert.equal(started.json.title, 'Second');
  const card = (await call('GET', `/projects/${p.id}/cards`)).json.find((c: any) => c.id === b.id);
  assert.equal(card.sessionId, started.json.id);
  assert.equal(card.column, 'doing');
  assert.equal((await feed(started.json.id))[0].text, 'Second\n\nDetails', 'prompt = title + description');
  await waitFor(started.json.id, (x) => x.status !== 'running' && x.status !== 'waiting', 'the card turn');

  assert.deepEqual((await call('DELETE', `/cards/${a.json.id}`)).json, { ok: true });
  assert.equal((await call('GET', `/projects/${p.id}/cards`)).json.length, 1);
});

test('project manifest .hub/hub.yaml', async () => {
  const repo = makeRepo(join(tmp, 'elsewhere', 'manifest'));
  const [p] = (await call('POST', '/projects/import', { paths: [repo] })).json;
  const path = join(repo, '.hub', 'hub.yaml');
  assert.deepEqual((await call('GET', `/projects/${p.id}/manifest`)).json, { exists: false, path, manifest: null, services: [] });
  const created = await call('POST', `/projects/${p.id}/manifest`);
  assert.equal(created.status, 201);
  assert.deepEqual(created.json, {
    exists: true,
    path,
    manifest: { links: {}, ssh: [], services: [], units: [], docs: ['AGENTS.md'], notes: 'Anything agents should always know about this project.\n' },
    services: [],
  });
  writeFileSync(path, 'links:\n  grafana: http://localhost:3000\n  port: 8080\nssh: prod\nservices: [hubness-no-such-container]\n');
  const m = (await call('GET', `/projects/${p.id}/manifest`)).json;
  assert.deepEqual(m.manifest, { links: { grafana: 'http://localhost:3000', port: '8080' }, ssh: ['prod'], services: ['hubness-no-such-container'], units: [], docs: [] });
  assert.deepEqual(m.services, [{ name: 'hubness-no-such-container', state: 'missing', status: 'not found' }]);
  assert.equal((await call('POST', `/projects/${p.id}/manifest`)).json.manifest.ssh[0], 'prod', 'an existing manifest is kept');
  writeFileSync(path, 'links: [unclosed\n');
  const broken = (await call('GET', `/projects/${p.id}/manifest`)).json;
  assert.equal(broken.exists, true);
  assert.equal(broken.manifest, null);
  assert.equal(typeof broken.error, 'string');
  assert.equal((await call('GET', '/projects/missing/manifest')).status, 404);
});

test('files: tree, read, write with conflict check, create/rename/delete, list, browse', async () => {
  const { mkdirSync, symlinkSync } = await import('node:fs');
  const repo = makeRepo(join(tmp, 'elsewhere', 'files'));
  mkdirSync(join(repo, 'src'));
  mkdirSync(join(repo, 'dist'));
  writeFileSync(join(repo, '.gitignore'), 'dist/\n');
  writeFileSync(join(repo, 'b.txt'), 'hello\n');
  writeFileSync(join(repo, 'A.md'), '# Title\n');
  writeFileSync(join(repo, 'src', 'main.ts'), 'export {};\n');
  writeFileSync(join(repo, 'blob.bin'), Buffer.from([1, 0, 2]));
  writeFileSync(join(repo, 'dist', 'out.js'), '');
  symlinkSync(tmp, join(repo, 'escape'));
  const [p] = (await call('POST', '/projects/import', { paths: [repo] })).json;
  const base = `/projects/${p.id}`;

  const tree = (await call('GET', `${base}/tree`)).json;
  assert.deepEqual(
    tree.map((e: any) => [e.name, e.dir, e.ignored]),
    [
      ['dist', true, true],
      ['escape', true, false],
      ['src', true, false],
      ['.gitignore', false, false],
      ['A.md', false, false],
      ['b.txt', false, false],
      ['blob.bin', false, false],
    ],
  );
  assert.equal(tree.find((e: any) => e.name === 'b.txt').size, 6);
  assert.equal('size' in tree.find((e: any) => e.name === 'src'), false, 'folders have no size');
  assert.deepEqual((await call('GET', `${base}/tree?path=src`)).json, [{ name: 'main.ts', path: 'src/main.ts', dir: false, size: 11, ignored: false }]);
  assert.equal((await call('GET', `${base}/tree?path=..`)).status, 400);
  assert.equal((await call('GET', `${base}/tree?path=escape`)).status, 400, 'a symlink out of the project');

  const file = (await call('GET', `${base}/file?path=b.txt`)).json;
  assert.deepEqual(file, { path: 'b.txt', size: 6, binary: false, truncated: false, content: 'hello\n', hash: '5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03' });
  const bin = (await call('GET', `${base}/file?path=blob.bin`)).json;
  assert.deepEqual(bin, { path: 'blob.bin', size: 3, binary: true, truncated: false, content: '' });
  assert.equal((await call('GET', `${base}/file`)).status, 400);
  assert.equal((await call('GET', `${base}/file?path=src`)).status, 400, 'a folder');
  assert.equal((await call('GET', `${base}/file?path=nope.txt`)).status, 404);
  assert.equal((await call('GET', `${base}/file?path=escape/claude-fixture`)).status, 400);

  assert.equal((await call('PUT', `${base}/file`, { path: 'b.txt', content: 'x', baseHash: 'stale' })).status, 409);
  const saved = await call('PUT', `${base}/file`, { path: 'b.txt', content: 'bye\n', baseHash: file.hash });
  assert.equal(saved.status, 200);
  assert.equal(saved.json.content, 'bye\n');
  assert.notEqual(saved.json.hash, file.hash);
  assert.equal((await call('PUT', `${base}/file`, { path: '.git/config', content: 'x' })).status, 400);
  assert.equal((await call('PUT', `${base}/file`, { path: 'nope.txt', content: 'x' })).status, 404);
  assert.equal((await call('PUT', `${base}/file`, { path: 'b.txt', content: 'a\u0000b' })).status, 400);
  assert.equal((await call('PUT', `${base}/file`, { path: 'blob.bin', content: 'text' })).status, 400, 'binary files stay binary');
  assert.equal((await call('PUT', `${base}/file`, { path: '', content: 'x' })).status, 400);

  assert.deepEqual((await call('POST', `${base}/fs/create`, { path: 'docs/notes.md' })).json, { path: 'docs/notes.md' });
  assert.equal((await call('POST', `${base}/fs/create`, { path: 'docs/notes.md' })).status, 409);
  assert.equal((await call('POST', `${base}/fs/create`, { path: 'lib', dir: true })).status, 201);
  assert.equal((await call('POST', `${base}/fs/create`, { path: '../out' })).status, 400);
  assert.equal((await call('POST', `${base}/fs/create`, { path: '.git/hooks/x' })).status, 400);
  assert.equal((await call('POST', `${base}/fs/create`, { path: 'escape/x' })).status, 400, 'not through a symlink');
  assert.deepEqual((await call('POST', `${base}/fs/rename`, { from: 'docs', to: 'lib/docs' })).json, { path: 'lib/docs' });
  assert.equal((await call('POST', `${base}/fs/rename`, { from: 'lib', to: 'lib/inner' })).status, 400, 'into itself');
  assert.equal((await call('POST', `${base}/fs/rename`, { from: 'A.md', to: 'b.txt' })).status, 409, 'never overwrites');
  assert.equal((await call('POST', `${base}/fs/rename`, { from: 'missing', to: 'x' })).status, 404);
  assert.deepEqual((await call('POST', `${base}/fs/delete`, { path: 'lib' })).json, { ok: true });
  assert.deepEqual((await call('POST', `${base}/fs/delete`, { path: 'escape' })).json, { ok: true }, 'the link itself is removed');
  assert.ok(existsSync(tmp), 'not its target');
  assert.equal((await call('POST', `${base}/fs/delete`, { path: 'lib' })).status, 404);

  const files = (await call('GET', `${base}/files`)).json.files.sort();
  assert.deepEqual(files, ['.gitignore', 'A.md', 'b.txt', 'blob.bin', 'src/main.ts'], 'ignored files are left out');

  const br = (await call('GET', `/fs/browse?path=${encodeURIComponent(repo)}`)).json;
  assert.equal(br.path, repo);
  assert.equal(br.parent, join(tmp, 'elsewhere'));
  assert.equal(br.isGitRepo, true);
  assert.deepEqual(br.entries.map((e: any) => e.name), ['dist', 'src']);
  assert.equal((await call('GET', `/fs/browse?path=${encodeURIComponent(join(repo, 'b.txt'))}`)).status, 400);
});

test('search across project files', async () => {
  const { mkdirSync } = await import('node:fs');
  const repo = makeRepo(join(tmp, 'elsewhere', 'search'));
  mkdirSync(join(repo, 'src'));
  writeFileSync(join(repo, 'src', 'a.ts'), 'const Needle = 1;\nneedle();\n// needles\n');
  writeFileSync(join(repo, 'notes.md'), 'привет needle\n');
  writeFileSync(join(repo, '.gitignore'), 'ignored.txt\n');
  writeFileSync(join(repo, 'ignored.txt'), 'needle\n');
  const [p] = (await call('POST', '/projects/import', { paths: [repo] })).json;
  const search = async (qs: string) => {
    const r = (await call('GET', `/projects/${p.id}/search?${qs}`)).json;
    r.files.sort((a: any, b: any) => a.path.localeCompare(b.path));
    return r;
  };
  assert.deepEqual(await search('q=%20'), { files: [], total: 0, truncated: false });
  const all = await search('q=needle');
  assert.equal(all.total, 4);
  assert.deepEqual(all.files.map((f: any) => f.path), ['notes.md', 'src/a.ts'], '.gitignore is respected');
  assert.deepEqual(all.files[0].matches, [{ line: 1, text: 'привет needle', ranges: [[7, 13]] }], 'UTF-16 ranges');
  assert.equal((await search('q=Needle&case=1')).total, 1);
  assert.equal((await search('q=needle&word=1')).total, 3);
  assert.equal((await search('q=need(le)%3F%5Cb&regex=1')).total, 3);
  assert.deepEqual((await search('q=needle&include=*.md')).files.map((f: any) => f.path), ['notes.md']);
  assert.deepEqual((await search('q=needle&exclude=*.md')).files.map((f: any) => f.path), ['src/a.ts']);
  assert.equal((await call('GET', `/projects/${p.id}/search?q=(&regex=1`)).status, 400);
  assert.equal((await call('GET', `/projects/${p.id}/search?q=${'x'.repeat(501)}`)).status, 400);
});

test('live file changes over /watch', async () => {
  const { execFileSync } = await import('node:child_process');
  const repo = makeRepo(join(tmp, 'elsewhere', 'watched'));
  const [p] = (await call('POST', '/projects/import', { paths: [repo] })).json;
  assert.equal((await raw('/api/watch?projects=missing')).status, 404);
  const stream = await openStream(`/watch?projects=${p.id},${p.id}`);
  await new Promise((r) => setTimeout(r, 100));
  writeFileSync(join(repo, 'new.txt'), 'x');
  const first = await stream.next();
  assert.equal(first.projectId, p.id);
  assert.ok(first.paths.includes('new.txt'), JSON.stringify(first));
  const { mkdirSync } = await import('node:fs');
  mkdirSync(join(repo, 'pkg'));
  await new Promise((r) => setTimeout(r, 300));
  writeFileSync(join(repo, 'pkg', 'deep.txt'), 'x');
  let m = await stream.next();
  while (!m.paths.includes('pkg/deep.txt')) m = await stream.next();
  execFileSync('git', ['-C', repo, 'add', 'new.txt']);
  m = await stream.next();
  while (!m.git) m = await stream.next();
  assert.equal(m.git, true, 'staging changes the index');
  stream.close();
});

test('ACP agents: codex turn (bypass and ask with approval), opencode provider error', async () => {
  useFixture('acp-tool-turn.jsonl');
  const s = (await call('POST', '/sessions', { agent: { kind: 'codex', permissionMode: 'bypass' }, prompt: 'fix the notes' })).json;
  const done = await waitFor(s.id, (x) => x.status === 'idle', 'the codex turn');
  assert.equal(done.agentSessionId, 'acp-sess-1');
  const events = (await feed(s.id)).map(({ durationMs: _d, toolId, ...e }: any) => ({ ...e, ...(toolId ? { toolId: toolId.replace(/^plan:.*/, 'plan:<run>') } : {}) }));
  assert.deepEqual(events, [
    { uid: 'u', kind: 'user', text: 'fix the notes' },
    { uid: 'th', kind: 'thinking', text: 'Need to look at the file.' },
    { uid: 't', kind: 'text', text: 'Let me edit it.', streaming: false },
    { uid: 'tool', kind: 'tool', name: 'Bash', input: { command: 'cat notes.txt' }, status: 'ok', output: 'hello', toolId: 'call_1' },
    { uid: 'tool', kind: 'tool', name: 'Edit', input: { file_path: '/work/notes.txt', old_string: 'hello', new_string: 'hello world' }, status: 'ok', output: 'diff /work/notes.txt', toolId: 'call_2' },
    { uid: 'plan', kind: 'tool', name: 'TodoWrite', input: { todos: [{ content: 'Read notes', status: 'completed' }, { content: 'Edit notes', status: 'completed' }] }, status: 'ok', toolId: 'plan:<run>' },
    { uid: 't', kind: 'text', text: 'Done: notes.txt now says hello world.', streaming: false },
    { uid: 'res', kind: 'result', ok: true, model: 'provider/model-a' },
  ]);

  const asked = (await call('POST', '/sessions', { agent: { kind: 'codex', permissionMode: 'ask' }, prompt: 'fix the notes' })).json;
  await waitFor(asked.id, (x) => x.status === 'waiting', 'the codex permission request');
  const ap = (await feed(asked.id)).find((e: any) => e.kind === 'approval');
  assert.equal(ap.toolName, 'Bash');
  assert.deepEqual(ap.input, { command: 'cat notes.txt' });
  assert.match(ap.requestId, /^0:/);
  assert.deepEqual((await call('POST', `/sessions/${asked.id}/approvals/${ap.requestId}`, { decision: 'allow' })).json, { ok: true });
  await waitFor(asked.id, (x) => x.status === 'idle', 'the codex turn after approval');
  const after = await feed(asked.id);
  assert.equal(after.find((e: any) => e.kind === 'approval').status, 'allowed');
  assert.equal(after.at(-1).kind, 'result');

  useFixture('acp-provider-error.jsonl');
  const oc = (await call('POST', '/sessions', { agent: { kind: 'opencode' }, prompt: 'hi' })).json;
  const failed = await waitFor(oc.id, (x) => x.status !== 'running' && x.status !== 'waiting', 'the opencode turn');
  assert.equal(failed.status, 'error');
  assert.equal(failed.agentSessionId, 'acp-sess-2');
  assert.deepEqual((await feed(oc.id)).slice(1), [
    { uid: 'retry', kind: 'notice', level: 'info', text: 'Provider error 429 model:free is temporarily rate-limited upstream. — retry #1' },
    { uid: 'err', kind: 'notice', level: 'error', text: 'Provider request failed\n429 model:free is temporarily rate-limited upstream.' },
  ]);
});

test('hub MCP server: open_pane and close_pane reach the session (rust: `hubness mcp`)', { skip: target !== 'rust' && 'the TS server runs in-process (no listener)' }, async () => {
  const s = (await call('POST', '/sessions', { agent: { kind: 'claude-code' } })).json;
  const bin = process.env.HUBNESS_RUST_BIN ?? join(repoRoot, 'target/debug/hubness');
  const mcp = spawn(bin, ['mcp'], { env: { ...process.env, HUB_URL: rustBase, HUB_SESSION_ID: s.id }, stdio: ['pipe', 'pipe', 'inherit'] });
  const { createInterface } = await import('node:readline');
  const replies = createInterface({ input: mcp.stdout! })[Symbol.asyncIterator]();
  const rpc = async (id: number, method: string, params: unknown = {}) => {
    mcp.stdin!.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
    return JSON.parse((await replies.next()).value as string);
  };
  assert.equal((await rpc(1, 'initialize', { protocolVersion: '2025-06-18' })).result.serverInfo.name, 'hubness');
  mcp.stdin!.write(JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' }) + '\n');
  assert.deepEqual((await rpc(2, 'tools/list')).result.tools.map((t: any) => t.name), ['open_pane', 'close_pane']);
  const bad = await rpc(3, 'tools/call', { name: 'open_pane', arguments: { kind: 'video', target: 'x' } });
  assert.equal(bad.result.isError, true);
  const opened = await rpc(4, 'tools/call', { name: 'open_pane', arguments: { kind: 'url', target: 'http://localhost:5173', title: 'App' } });
  assert.deepEqual(opened.result, { content: [{ type: 'text', text: 'Opened url panel: http://localhost:5173' }], isError: false });
  const live = await openStream(`/sessions/${s.id}/stream`);
  const snap = await live.next();
  assert.equal(snap.pane.target, 'http://localhost:5173');
  assert.equal(snap.pane.title, 'App');
  assert.equal((await rpc(5, 'tools/call', { name: 'close_pane', arguments: {} })).result.isError, false);
  let m = await live.next();
  while (m.type !== 'pane') m = await live.next();
  assert.equal(m.pane, null);
  assert.equal((await rpc(6, 'nope')).error.code, -32601);
  live.close();
  mcp.stdin!.end();
});

test('attachments: upload, serve, attach to a message', async () => {
  const repo = makeRepo(join(tmp, 'elsewhere', 'attach'));
  const [p] = (await call('POST', '/projects/import', { paths: [repo] })).json;
  const inProject = (await call('POST', '/sessions', { projectId: p.id, agent: { kind: 'claude-code' } })).json;
  const png = Buffer.from('89504e470d0a1a0a0000000d4948445200000001000000010806000000', 'hex');
  const upload = (id: string, name: string, type: string, body: Uint8Array) =>
    raw(`/api/sessions/${id}/attachments`, { method: 'POST', headers: { 'X-Hubness-Request': '1', 'Content-Type': type, 'X-Filename': encodeURIComponent(name) }, body });
  const up = await upload(inProject.id, 'скрин (1).png', 'image/png', png);
  assert.equal(up.status, 201);
  const a = (await up.json()) as any;
  assert.match(a.id, /^[0-9a-f]{8}-скрин _1_\.png$/);
  assert.equal(a.name, 'скрин _1_.png');
  assert.equal(a.mime, 'image/png');
  assert.equal(a.size, png.length);
  assert.equal(a.path, join(repo, '.hub', 'attachments', a.id));
  const { readFileSync } = await import('node:fs');
  assert.match(readFileSync(join(repo, '.git', 'info', 'exclude'), 'utf8'), /^\/\.hub\/attachments\/$/m, 'kept out of git locally');
  assert.equal((await upload(inProject.id, 'empty.txt', 'text/plain', new Uint8Array())).status, 400);
  const txt = (await (await upload(inProject.id, 'notes.md', 'bogus', Buffer.from('# hi'))).json()) as any;
  assert.equal(txt.mime, 'text/markdown', 'a bad type falls back to the extension');

  const served = await raw(`/api/sessions/${inProject.id}/attachments/${encodeURIComponent(a.id)}`);
  assert.equal(served.status, 200);
  assert.equal(served.headers.get('content-type'), 'image/png');
  assert.equal(served.headers.get('content-disposition'), 'inline');
  assert.equal(served.headers.get('x-content-type-options'), 'nosniff');
  assert.deepEqual(Buffer.from(await served.arrayBuffer()), png);
  const md = await raw(`/api/sessions/${inProject.id}/attachments/${encodeURIComponent(txt.id)}`);
  assert.equal(md.headers.get('content-disposition'), 'attachment', 'non-media is a download');
  assert.equal((await raw(`/api/sessions/${inProject.id}/attachments/..%2Fsecret`)).status, 404);
  assert.equal((await raw(`/api/sessions/${inProject.id}/attachments/missing`)).status, 404);

  useFixture('claude-tool-turn.jsonl');
  const chat = (await call('POST', '/sessions', { agent: { kind: 'claude-code' } })).json;
  const shot = (await (await upload(chat.id, 'shot.png', 'image/png', png)).json()) as any;
  assert.equal((await call('POST', `/sessions/${chat.id}/messages`, { text: '', attachments: [{ id: 'nope' }] })).status, 404);
  assert.equal((await call('POST', `/sessions/${chat.id}/messages`, { text: '', attachments: 'x' })).status, 400);
  const sent = await call('POST', `/sessions/${chat.id}/messages`, { text: '', attachments: [{ id: shot.id, name: shot.name, mime: shot.mime }] });
  assert.equal(sent.status, 200);
  assert.equal(sent.json.title, 'shot.png', 'an attachment names a chat without text');
  await waitFor(chat.id, (x) => x.status === 'idle' && x.agentSessionId, 'the turn with an attachment');
  const user = (await feed(chat.id))[0];
  assert.deepEqual(user.attachments, [{ id: shot.id, name: 'shot.png', mime: 'image/png', size: png.length, path: shot.path }]);
});

test('inbox chat → project: suggestion and conversion', async () => {
  useFixture('claude-tool-turn.jsonl');
  const chat = (await call('POST', '/sessions', { agent: { kind: 'claude-code' }, prompt: 'Build my app' })).json;
  await waitFor(chat.id, (x) => x.status === 'idle' && x.agentSessionId, 'the first turn');
  assert.deepEqual((await call('GET', `/sessions/${chat.id}/project-suggestion`)).json, { suggest: false, reason: null, name: 'build-my-app' });
  writeFileSync(join(chat.cwd, 'package.json'), '{}');
  assert.deepEqual((await call('GET', `/sessions/${chat.id}/project-suggestion`)).json, { suggest: true, reason: 'found package.json', name: 'build-my-app' });

  const { mkdirSync } = await import('node:fs');
  mkdirSync(join(tmp, 'hub', 'projects', 'taken'), { recursive: true });
  assert.equal((await call('POST', `/sessions/${chat.id}/convert`, { name: 'Taken' })).status, 409);
  const res = await call('POST', `/sessions/${chat.id}/convert`, { name: 'Мой App' });
  assert.equal(res.status, 200);
  const dest = join(tmp, 'hub', 'projects', 'moy-app');
  assert.equal(res.json.project.path, dest);
  assert.equal(res.json.project.name, 'Мой App');
  assert.equal(res.json.session.projectId, res.json.project.id);
  assert.equal(res.json.session.cwd, null);
  assert.equal(res.json.session.agentSessionId, null, 'the fake agent has no transcript to carry: a summary instead');
  for (const f of ['package.json', 'AGENTS.md', '.git', '.hub/hub.yaml']) assert.ok(existsSync(join(dest, f)), f);
  assert.ok(!existsSync(chat.cwd), 'the inbox folder moved');
  assert.equal((await call('POST', `/sessions/${chat.id}/convert`, {})).status, 400, 'only inbox chats');
  assert.deepEqual((await call('GET', `/sessions/${chat.id}/project-suggestion`)).json.suggest, false);
});

// ---------- git ----------

const gitIn = (dir: string, ...args: string[]) => execFileSync('git', ['-C', dir, ...args], { encoding: 'utf8' }).trim();

/** A repository with an author and one commit ("init" with a.txt and old.txt). */
function repoWithCommit(path: string, remote?: string): string {
  makeRepo(path, remote);
  gitIn(path, 'config', 'user.email', 'test@example.com');
  gitIn(path, 'config', 'user.name', 'Test');
  writeFileSync(join(path, 'a.txt'), 'one\n');
  writeFileSync(join(path, 'old.txt'), 'rename me\n');
  gitIn(path, 'add', '-A');
  gitIn(path, 'commit', '-q', '-m', 'init');
  return path;
}

const byPath = (files: any[]) => [...files].sort((a, b) => (a.path < b.path ? -1 : 1));

test('git: status, diff, stage, commit, log and show', async () => {
  const repo = repoWithCommit(join(tmp, 'elsewhere', 'git-status'));
  const [p] = (await call('POST', '/projects/import', { paths: [repo] })).json;
  const git = (path: string) => `/projects/${p.id}/git${path}`;

  writeFileSync(join(repo, 'a.txt'), 'one\ntwo\n');
  gitIn(repo, 'mv', 'old.txt', 'new name.txt');
  writeFileSync(join(repo, 'staged.txt'), 's\n');
  gitIn(repo, 'add', 'staged.txt');
  writeFileSync(join(repo, 'untracked file.txt'), 'u\n');

  const st = (await call('GET', git('/status'))).json;
  assert.deepEqual({ ...st, files: byPath(st.files) }, {
    isRepo: true,
    branch: 'main',
    upstream: null,
    ahead: 0,
    behind: 0,
    detached: false,
    files: [
      { path: 'a.txt', index: ' ', worktree: 'M', untracked: false, conflicted: false },
      { path: 'new name.txt', index: 'R', worktree: ' ', untracked: false, conflicted: false, from: 'old.txt' },
      { path: 'staged.txt', index: 'A', worktree: ' ', untracked: false, conflicted: false },
      { path: 'untracked file.txt', index: ' ', worktree: '?', untracked: true, conflicted: false },
    ],
    state: null,
    stashes: 0,
  });
  assert.equal((await call('GET', '/projects/missing/git/status')).status, 404);
  const notRepo = (await call('GET', `/projects/${(await call('GET', '/projects')).json.find((x: any) => x.kind === 'system').id}/git/status`)).json;
  assert.equal(notRepo.isRepo, false);

  const unstaged = (await call('GET', git('/diff?path=a.txt'))).json.diff;
  assert.match(unstaged, /^\+two$/m);
  const staged = (await call('GET', git('/diff?staged=1'))).json.diff;
  assert.match(staged, /^\+s$/m);
  assert.doesNotMatch(staged, /\+two/);
  const untracked = (await call('GET', git(`/diff?path=${encodeURIComponent('untracked file.txt')}&untracked=1`))).json.diff;
  assert.match(untracked, /new file mode/);
  assert.match(untracked, /^\+u$/m);

  assert.equal((await call('POST', git('/stage'), {})).status, 400, 'paths are required');
  assert.equal((await call('POST', git('/stage'), { paths: [] })).status, 400);
  assert.equal((await call('POST', git('/stage'), { paths: 'a.txt' })).status, 400, 'a mistyped field');
  assert.equal((await call('POST', git('/nope'), {})).status, 404, 'unknown operation');
  assert.equal((await call('GET', git('/nope'))).status, 404);
  assert.equal((await call('POST', '/projects/missing/git/commit', {})).status, 404);
  assert.equal((await call('POST', git('/stage'), { paths: ['a.txt'] })).json.ok, true);
  assert.equal(byPath((await call('GET', git('/status'))).json.files)[0].index, 'M');
  assert.equal((await call('POST', git('/unstage'), { paths: ['a.txt'] })).json.ok, true);
  assert.deepEqual(byPath((await call('GET', git('/status'))).json.files)[0], { path: 'a.txt', index: ' ', worktree: 'M', untracked: false, conflicted: false });

  // no body / a broken body counts as {} for operations
  assert.deepEqual((await call('POST', git('/commit'))).json, { ok: false, output: 'Empty commit message' });
  assert.deepEqual((await call('POST', git('/continue'), '{oops')).json, { ok: false, output: 'Nothing in progress' });
  const committed = await call('POST', git('/commit'), { message: 'second' });
  assert.equal(committed.status, 200);
  assert.equal(committed.json.ok, true);
  assert.match(committed.json.output, /second/);
  assert.deepEqual((await call('GET', git('/status'))).json.files, [], 'all: true by default');

  const log = (await call('GET', git('/log'))).json;
  assert.equal(log.more, false);
  assert.deepEqual(log.commits.map((c: any) => c.subject), ['second', 'init']);
  const [second, init] = log.commits;
  assert.equal(second.parents[0], init.hash);
  assert.deepEqual(init.parents, []);
  assert.equal(second.short, second.hash.slice(0, second.short.length));
  assert.equal(second.author, 'Test');
  assert.deepEqual(second.refs, ['HEAD -> main']);
  assert.deepEqual(init.refs, []);
  const limited = (await call('GET', git('/log?limit=1'))).json;
  assert.equal(limited.commits.length, 1);
  assert.equal(limited.more, true);
  assert.equal((await call('GET', git('/log?limit=abc'))).json.commits.length, 2);

  assert.match((await call('GET', git(`/show?rev=${second.hash}`))).json.diff, /^\+two$/m);
  assert.deepEqual((await call('GET', git('/show?rev=-p'))).json, { diff: '' }, 'options are not revisions');
  assert.deepEqual((await call('POST', git('/reset'), { rev: 'HEAD', mode: 'bogus' })).status, 400);
  assert.deepEqual((await call('POST', git('/reset'), { rev: '--hard' })).json, { ok: false, output: 'Invalid commit' });
});

test('git: branches, checkout, merge and a local remote', async () => {
  const repo = repoWithCommit(join(tmp, 'elsewhere', 'git-branches'));
  const [p] = (await call('POST', '/projects/import', { paths: [repo] })).json;
  const git = (path: string) => `/projects/${p.id}/git${path}`;
  const init = gitIn(repo, 'rev-parse', 'HEAD');

  assert.equal((await call('POST', git('/checkout'), { branch: '  ' })).status, 400, 'branch is required');
  assert.equal((await call('POST', git('/checkout'), { branch: ' feature/x ', create: true })).json.ok, true);
  assert.equal((await call('GET', git('/status'))).json.branch, 'feature/x');
  writeFileSync(join(repo, 'feature.txt'), 'f\n');
  assert.equal((await call('POST', git('/commit'), { message: 'feature work' })).json.ok, true);

  const branches = (await call('GET', git('/branches'))).json;
  assert.deepEqual(branches.map((b: any) => b.name).sort(), ['feature/x', 'main']);
  const current = branches.find((b: any) => b.current);
  assert.equal(current.name, 'feature/x');
  assert.equal(current.remote, false);
  assert.equal(current.upstream, null);
  assert.equal(typeof current.lastCommit, 'string');

  assert.equal((await call('POST', git('/checkout'), { branch: 'main' })).json.ok, true);
  assert.equal((await call('POST', git('/checkout'), { branch: 'no-such-branch' })).json.ok, false);
  assert.equal((await call('POST', git('/merge'), { branch: 'feature/x', noFf: true })).json.ok, true);
  assert.equal((await call('GET', git('/log?all=0'))).json.commits[0].parents.length, 2, 'a merge commit');

  assert.equal((await call('POST', git('/branch-at'), { branch: 'from-init', rev: init })).json.ok, true);
  assert.equal((await call('GET', git('/status'))).json.branch, 'from-init');
  assert.equal((await call('POST', git('/checkout'), { branch: 'main' })).json.ok, true);
  assert.equal((await call('POST', git('/delete-branch'), { branch: 'from-init' })).json.ok, true);
  assert.deepEqual((await call('POST', git('/delete-branch'), { branch: '-D' })).json, { ok: false, output: 'Invalid branch' });
  assert.deepEqual((await call('POST', git('/branch-at'), { branch: 'x', rev: 'a..b' })).json, { ok: false, output: 'Invalid branch or commit' });

  // a local bare repository as origin: push sets the upstream, fetch works
  const bare = join(tmp, 'elsewhere', 'git-branches-origin.git');
  execFileSync('git', ['init', '-q', '--bare', bare]);
  gitIn(repo, 'remote', 'add', 'origin', bare);
  const pushed = await call('POST', git('/push'));
  assert.equal(pushed.json.ok, true, pushed.json.output);
  assert.equal(pushed.json.auth, undefined);
  assert.equal((await call('GET', git('/status'))).json.upstream, 'origin/main');
  assert.equal((await call('POST', git('/fetch'))).json.ok, true);
  assert.ok((await call('GET', git('/branches'))).json.some((b: any) => b.name === 'origin/main' && b.remote));
});

test('git: stash push, list, show, pop and discard', async () => {
  const repo = repoWithCommit(join(tmp, 'elsewhere', 'git-stash'));
  const [p] = (await call('POST', '/projects/import', { paths: [repo] })).json;
  const git = (path: string) => `/projects/${p.id}/git${path}`;
  writeFileSync(join(repo, 'a.txt'), 'changed\n');
  writeFileSync(join(repo, 'new.txt'), 'n\n');

  assert.equal((await call('POST', git('/stash-push'), { message: ' wip ' })).json.ok, true);
  const st = (await call('GET', git('/status'))).json;
  assert.deepEqual(st.files, [], 'untracked files are stashed by default');
  assert.equal(st.stashes, 1);
  const list = (await call('GET', git('/stashes'))).json;
  assert.equal(list.length, 1);
  assert.equal(list[0].ref, 'stash@{0}');
  assert.equal(list[0].message, 'wip');
  assert.equal(typeof list[0].date, 'string');
  assert.match((await call('GET', git(`/show?rev=${encodeURIComponent('stash@{0}')}`))).json.diff, /^\+changed$/m);

  assert.equal((await call('POST', git('/stash'), { action: 'nope', rev: 'stash@{0}' })).status, 400);
  assert.deepEqual((await call('POST', git('/stash'), { action: 'drop', rev: 'HEAD' })).json, { ok: false, output: 'Invalid stash' });
  assert.equal((await call('POST', git('/stash'), { action: 'pop', rev: 'stash@{0}' })).json.ok, true);
  const popped = (await call('GET', git('/status'))).json;
  assert.equal(popped.stashes, 0);
  assert.deepEqual(byPath(popped.files).map((f: any) => f.path), ['a.txt', 'new.txt']);

  assert.equal((await call('POST', git('/discard'), { paths: ['a.txt', 'new.txt'] })).json.ok, true);
  assert.deepEqual((await call('GET', git('/status'))).json.files, []);
  assert.ok(!existsSync(join(repo, 'new.txt')), 'untracked files are deleted');
});

test('project detail: repository info, provider links and languages', async () => {
  const repo = repoWithCommit(join(tmp, 'elsewhere', 'widget'), 'https://user:tok3n@github.com/acme/widget.git');
  writeFileSync(join(repo, 'main.rs'), 'x'.repeat(300));
  writeFileSync(join(repo, 'app.ts'), 'y'.repeat(100));
  writeFileSync(join(repo, 'README.md'), '# prose is not counted\n');
  writeFileSync(join(repo, 'data.csv'), 'a,b\n');
  gitIn(repo, 'add', '-A');
  gitIn(repo, 'commit', '-q', '-m', 'code');
  gitIn(repo, 'checkout', '-q', '-b', 'feat/x');
  const [p] = (await call('POST', '/projects/import', { paths: [repo] })).json;
  assert.equal(p.gitProvider, null);

  const res = await call('GET', `/projects/${p.id}`);
  assert.equal(res.status, 200);
  const d = res.json;
  assert.ok(!JSON.stringify(d).includes('tok3n'), 'credentials are stripped from the remote URL');
  const { languages, ...repoInfo } = d.repo;
  assert.deepEqual(repoInfo, {
    isRepo: true,
    provider: 'github',
    providerLabel: 'GitHub',
    remoteUrl: 'https://github.com/acme/widget.git',
    branch: 'feat/x',
    defaultBranch: 'main',
    links: {
      repo: 'https://github.com/acme/widget',
      branch: 'https://github.com/acme/widget/tree/feat/x',
      newPullRequest: 'https://github.com/acme/widget/compare/main...feat/x?expand=1',
    },
  });
  assert.deepEqual(languages.map((l: any) => [l.name, l.percent]), [['Rust', 75], ['TypeScript', 25]]);
  assert.match(languages[0].color, /^#[0-9a-f]{6}$/i);
  assert.equal(d.gitProvider, 'github', 'repository info is cached in the project');
  assert.equal(d.repoUrl, 'https://github.com/acme/widget.git');
  assert.equal(d.defaultBranch, 'main');
  assert.deepEqual(d.language, { name: 'Rust', color: languages[0].color });
  assert.deepEqual(d.groups, []);
  assert.equal(d.workspace.scope, 'project');
  assert.equal(d.workspace.path, repo);
  assert.equal(d.missing, undefined);
  assert.equal((await call('GET', '/projects')).json.find((x: any) => x.id === p.id).gitProvider, 'github');
  assert.equal((await call('GET', '/projects/missing')).status, 404);

  const g = (await call('POST', '/groups', { name: 'Widgets', projectIds: [p.id] })).json;
  assert.deepEqual((await call('GET', `/projects/${p.id}`)).json.groups, [{ id: g.id, name: 'Widgets' }]);
  await call('DELETE', `/groups/${g.id}`);
});

test('group context preview refreshes repository info', async () => {
  const repo = repoWithCommit(join(tmp, 'elsewhere', 'ctx-repo'));
  const [p] = (await call('POST', '/projects/import', { paths: [repo] })).json;
  await call('PATCH', `/projects/${p.id}`, { description: 'Context service', instructions: 'Run tests' });
  gitIn(repo, 'remote', 'add', 'origin', 'git@gitlab.com:acme/ctx-repo.git');
  const g = (await call('POST', '/groups', { name: 'Ctx', description: 'One product', projectIds: [p.id] })).json;
  const res = await call('GET', `/groups/${g.id}/context`);
  assert.equal(res.status, 200);
  const ctx: string = res.json.context;
  assert.match(ctx, /^You are working on "Ctx" — a group of 1 repositories/);
  assert.ok(ctx.includes('## Group\nOne product'));
  assert.ok(ctx.includes(`- ctx-repo — path: ${repo}, default branch: main, remote: git@gitlab.com:acme/ctx-repo.git (GitLab)`), ctx);
  assert.ok(ctx.includes('### ctx-repo\nContext service\nInstructions: Run tests'));
  assert.equal((await call('GET', '/groups/missing/context')).status, 404);
  await call('DELETE', `/groups/${g.id}`);
});

test('git logins per host: the secret never leaves the server', async () => {
  assert.deepEqual((await call('GET', '/git-credentials')).json, []);
  const body = { username: ' me ', kind: 'token', secret: 'glpat-s3cr3t' };
  assert.equal((await call('PUT', '/git-credentials/bad_host', body)).status, 400);
  assert.equal((await call('PUT', '/git-credentials/gitlab.example.com', { ...body, kind: 'ssh' })).status, 400);
  assert.equal((await call('PUT', '/git-credentials/gitlab.example.com', { ...body, username: '  ' })).status, 400);
  assert.equal((await call('PUT', '/git-credentials/gitlab.example.com', { ...body, secret: '' })).status, 400);
  assert.equal((await call('PUT', '/git-credentials/gitlab.example.com', { ...body, secret: 'a\nb' })).status, 400);
  assert.equal((await call('PUT', '/git-credentials/gitlab.example.com', { kind: 'token', secret: 'x' })).status, 400);

  const saved = await call('PUT', '/git-credentials/GitLab.Example.com', body);
  assert.equal(saved.status, 200);
  assert.equal(saved.json.length, 1);
  const [c] = saved.json;
  assert.deepEqual({ ...c, updatedAt: undefined }, { host: 'gitlab.example.com', username: 'me', kind: 'token', updatedAt: undefined });
  assert.match(c.updatedAt, /^\d{4}-\d\d-\d\dT/);
  assert.ok(!JSON.stringify(saved.json).includes('s3cr3t'));
  await call('PUT', '/git-credentials/codeberg.org', { username: 'u', kind: 'password', secret: 'pw' });
  const listed = (await call('GET', '/git-credentials')).json;
  assert.deepEqual(listed.map((x: any) => [x.host, x.kind]), [['codeberg.org', 'password'], ['gitlab.example.com', 'token']]);
  assert.ok(!JSON.stringify(listed).includes('s3cr3t') && !JSON.stringify(listed).includes('"pw"'));

  assert.deepEqual((await call('DELETE', '/git-credentials/gitlab.example.com')).json.map((x: any) => x.host), ['codeberg.org']);
  assert.equal((await call('DELETE', '/git-credentials/bad_host')).status, 200, 'an invalid host deletes nothing');
  assert.deepEqual((await call('DELETE', '/git-credentials/codeberg.org')).json, []);
});

test('new empty repository in the projects folder or a group folder', async () => {
  const created = await call('POST', '/projects/new-repo', { name: '  My Repo!  ' });
  assert.equal(created.status, 201);
  assert.equal(created.json.name, 'My-Repo');
  assert.equal(created.json.path, join(projectsRoot, 'My-Repo'));
  assert.equal(created.json.kind, 'normal');
  assert.ok(existsSync(join(projectsRoot, 'My-Repo', '.git')));
  assert.equal((await call('POST', '/projects/new-repo', { name: 'My Repo' })).status, 409, 'the folder exists');
  assert.equal((await call('POST', '/projects/new-repo', { name: '!!!' })).status, 400);
  assert.equal((await call('POST', '/projects/new-repo', {})).status, 400);
  assert.equal((await call('POST', '/projects/new-repo', { name: 5 })).status, 400);
  assert.equal((await call('POST', '/projects/new-repo', { name: 'x', groupId: 'missing' })).status, 404);

  const folder = join(tmp, 'group-store');
  const g = (await call('POST', '/groups', { name: 'Store', folder })).json;
  const inGroup = await call('POST', '/projects/new-repo', { name: 'svc', groupId: g.id });
  assert.equal(inGroup.status, 201);
  assert.equal(inGroup.json.path, join(folder, 'svc'));
  assert.deepEqual(inGroup.json.groupIds, [g.id]);
  assert.deepEqual((await call('GET', `/groups/${g.id}`)).json.projectIds, [inGroup.json.id]);
  const detail = (await call('GET', `/projects/${inGroup.json.id}`)).json;
  assert.equal(detail.repo.isRepo, true);
  assert.deepEqual(detail.repo.languages, []);
  assert.equal(detail.repo.provider, null);
  assert.equal(detail.repo.links, null);
  await call('DELETE', `/groups/${g.id}`);
});

test('pane file and diff of a session', async () => {
  const repo = repoWithCommit(join(tmp, 'elsewhere', 'pane'));
  const [p] = (await call('POST', '/projects/import', { paths: [repo] })).json;
  const s = (await call('POST', '/sessions', { projectId: p.id, agent: { kind: 'claude-code' } })).json;
  writeFileSync(join(repo, 'a.txt'), 'one\ntwo\n');
  writeFileSync(join(repo, 'fresh.txt'), 'new\n');
  gitIn(repo, 'add', 'fresh.txt');
  const file = (await call('GET', `/sessions/${s.id}/file?path=a.txt`)).json;
  assert.equal(file.content, 'one\ntwo\n');
  assert.equal((await call('GET', `/sessions/${s.id}/file?path=${encodeURIComponent(join(repo, 'a.txt'))}`)).json.content, 'one\ntwo\n', 'absolute inside the project');
  assert.equal((await call('GET', `/sessions/${s.id}/file?path=/etc/hostname`)).status, 400);
  const d = (await call('GET', `/sessions/${s.id}/diff?path=a.txt`)).json.diff;
  assert.match(d, /\+two/);
  const staged = (await call('GET', `/sessions/${s.id}/diff?path=fresh.txt`)).json.diff;
  assert.match(staged, /\+new/);
  assert.equal((await call('GET', `/sessions/missing/file?path=a.txt`)).status, 404);
});

test('harness: agents list, per-agent info, models, settings validation', async () => {
  const agents = (await call('GET', '/agents')).json;
  assert.deepEqual(agents.agents.map((a: any) => a.kind), ['opencode', 'claude-code', 'grok', 'codex']);
  assert.equal(agents.defaultKind, 'claude-code');
  for (const a of agents.agents) {
    assert.equal(typeof a.available, 'boolean');
    assert.equal(typeof a.detail, 'string');
    assert.ok(['options', 'search'].includes(a.modelStyle));
  }
  assert.deepEqual((await call('GET', '/harness/agents')).json.defaultKind, 'claude-code');
  assert.equal((await call('GET', '/agents/nope/models')).status, 404);
  assert.ok(Array.isArray((await call('GET', '/agents/codex/models')).json.models));
  for (const kind of ['codex', 'grok']) {
    const h = (await call('GET', `/harness/${kind}`)).json;
    assert.equal(typeof h.checkedAt, 'string');
    assert.equal(h.info.kind, kind);
    assert.equal(h.info.protocol, 'acp');
    assert.ok(['up-to-date', 'update-available', 'unknown', 'not-installed'].includes(h.info.version.status));
    assert.ok(Array.isArray(h.info.actions) && h.info.actions.every((x: any) => typeof x.command === 'string'));
    assert.equal(h.info.defaultModelEdit, null);
  }
  assert.equal((await call('GET', '/harness/nope')).status, 404);
  assert.equal((await call('PUT', '/harness/claude-code/default-model', { model: 'bad model!' })).status, 400);
  assert.equal((await call('PUT', '/harness/grok/default-model', { model: 'x' })).status, 404);
  assert.equal((await call('PUT', '/harness/nope/default-model', { model: 'x' })).status, 404);
  assert.equal((await call('POST', '/harness/claude-code/accounts/p/a/switch')).status, 404, 'no provider accounts');
  assert.equal((await call('POST', '/harness/opencode/accounts/p/a/delete')).status, 400);
  assert.equal((await call('POST', '/harness/opencode/accounts/bad%20id/a/switch')).status, 400);
});
