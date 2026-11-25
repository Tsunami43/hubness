import { makeRepo, tmp } from './setup.ts';
import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import { spawn, type ChildProcess } from 'node:child_process';
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
