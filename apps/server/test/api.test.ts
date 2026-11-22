import { makeRepo, tmp } from './setup.ts';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { join } from 'node:path';
import { Hono } from 'hono';
import { api } from '../src/routes/api.ts';
import { HttpError } from '../src/errors.ts';

// as in index.ts: HttpError → {error} with a status code
const app = new Hono();
app.onError((err, c) => c.json({ error: err.message }, err instanceof HttpError ? (err.status as 400) : 500));
app.route('/api', api);

const call = async (method: string, url: string, body?: unknown) => {
  const res = await app.request(`/api${url}`, {
    method,
    headers: body ? { 'Content-Type': 'application/json' } : undefined,
    body: body ? JSON.stringify(body) : undefined,
  });
  return { status: res.status, json: (await res.json()) as any };
};

const root = join(tmp, 'hub', 'projects'); // the fixed projects folder (HUBNESS_HOME/projects)
makeRepo(join(root, 'auth-service'), 'git@github.com:acme/auth-service.git');
makeRepo(join(root, 'billing-service'));

test('settings → discover → import → group → group session over HTTP', async () => {
  const normal = async () => (await call('GET', '/projects')).json.filter((p: any) => p.kind === 'normal');
  assert.equal((await normal()).length, 0, 'the projects folder is not auto-imported');
  const found = (await call('GET', '/projects/discover')).json;
  assert.equal(found.repos.length, 2);
  assert.equal((await call('POST', '/projects/import', { paths: [join(root, 'nope')] })).status, 400);
  const imported = await call('POST', '/projects/import', { paths: found.repos.map((r: any) => r.path) });
  assert.equal(imported.status, 201);
  assert.equal((await call('GET', '/projects/discover')).json.repos.length, 0, 'imported repos are no longer suggested');
  const ids = (await normal()).map((p: any) => p.id);
  assert.equal(ids.length, 2);

  const detail = await call('GET', `/projects/${ids[0]}`);
  assert.equal(detail.status, 200);
  assert.ok('repo' in detail.json && 'workspace' in detail.json);

  assert.equal((await call('POST', '/groups', { name: 'Bad', projectIds: ['missing'] })).status, 400);
  const g = await call('POST', '/groups', { name: 'Commerce Platform', projectIds: ids });
  assert.equal(g.status, 201);
  assert.equal(g.json.projects.length, 2);

  const both = await call('POST', '/sessions', { projectId: ids[0], groupId: g.json.id, agent: { kind: 'claude-code' } });
  assert.equal(both.status, 400);

  const s = await call('POST', '/sessions', { groupId: g.json.id, agent: { kind: 'opencode' } });
  assert.equal(s.status, 201);
  assert.equal(s.json.groupId, g.json.id);
  assert.equal(s.json.workspaceId, g.json.workspace.id);
  assert.deepEqual((await call('GET', `/sessions?groupId=${g.json.id}`)).json.map((x: any) => x.id), [s.json.id]);

  const ctx = await call('GET', `/groups/${g.json.id}/context`);
  assert.match(ctx.json.context, /Commerce Platform/);

  assert.equal((await call('GET', '/groups/missing')).status, 404);
  assert.equal((await call('DELETE', `/groups/${g.json.id}`)).status, 200);
  assert.equal((await call('GET', `/sessions/${s.json.id}`)).status, 404, 'group sessions removed with the group');
});

test('clone validates the URL before starting anything', async () => {
  assert.equal((await call('POST', '/projects/clone', { url: '-oProxyCommand=x' })).status, 400);
  assert.equal((await call('POST', '/projects/clone', { url: '/etc' })).status, 400);
});

test('removing a project keeps its files', async () => {
  const dir = makeRepo(join(root, 'scratch'));
  const [p] = (await call('POST', '/projects/import', { paths: [dir] })).json;
  assert.equal((await call('DELETE', `/projects/${p.id}`)).status, 200);
  assert.equal((await call('GET', `/projects/${p.id}`)).status, 404);
  const { existsSync } = await import('node:fs');
  assert.ok(existsSync(join(dir, '.git')));
});

test('default permission mode lives in hub settings', async () => {
  assert.equal((await call('GET', '/settings')).json.defaultPermission, 'ask', 'safe default');
  assert.equal((await call('PUT', '/settings', { defaultPermission: 'yolo' })).status, 400);
  const r = await call('PUT', '/settings', { defaultPermission: 'acceptEdits' });
  assert.equal(r.status, 200);
  assert.equal(r.json.defaultPermission, 'acceptEdits');
  assert.equal((await call('GET', '/settings')).json.defaultPermission, 'acceptEdits');
  assert.equal((await call('PUT', '/settings', {})).status, 400, 'empty patch');
  await call('PUT', '/settings', { defaultPermission: 'ask' });
});

test('session instructions live in hub settings and go into every session prompt', async () => {
  const { sessionSystemPrompt } = await import('../src/context.ts');
  assert.equal((await call('GET', '/settings')).json.sessionInstructions, '');
  assert.equal((await call('PUT', '/settings', { sessionInstructions: 42 })).status, 400);
  const r = await call('PUT', '/settings', { sessionInstructions: '  Answer in Russian.  ' });
  assert.equal(r.status, 200);
  assert.equal(r.json.sessionInstructions, 'Answer in Russian.', 'trimmed');
  assert.equal(sessionSystemPrompt('', null), null, 'chat without instructions: nothing to add');
  assert.match(sessionSystemPrompt('Answer in Russian.', null)!, /## General instructions\nAnswer in Russian\./);
  const both = sessionSystemPrompt('Answer in Russian.', '## Project instructions\nUse pnpm.')!;
  assert.ok(both.indexOf('General instructions') < both.indexOf('Project instructions'), 'general first, specific last');
  await call('PUT', '/settings', { sessionInstructions: '' });
});

test('delete a project with its files: guards, confirm by folder name, folder gone', async () => {
  const { existsSync } = await import('node:fs');
  const dir = makeRepo(join(tmp, 'elsewhere', 'doomed'));
  const [p] = (await call('POST', '/projects/import', { paths: [dir] })).json;
  assert.equal((await call('DELETE', `/projects/${p.id}?files=1`)).status, 400, 'no confirm');
  assert.equal((await call('DELETE', `/projects/${p.id}?files=1&confirm=wrong`)).status, 400, 'wrong confirm');
  assert.ok(existsSync(dir), 'nothing deleted on a failed confirm');

  // a folder holding another project is refused
  const outer = makeRepo(join(tmp, 'elsewhere', 'outer'));
  const inner = makeRepo(join(outer, 'inner'));
  const [o] = (await call('POST', '/projects/import', { paths: [outer] })).json;
  await call('POST', '/projects/import', { paths: [inner] });
  assert.equal((await call('DELETE', `/projects/${o.id}?files=1&confirm=outer`)).status, 409);
  assert.ok(existsSync(outer));

  const r = await call('DELETE', `/projects/${p.id}?files=1&confirm=doomed`);
  assert.equal(r.status, 200);
  assert.equal(r.json.deleted, dir);
  assert.ok(!existsSync(dir), 'folder deleted');
  assert.equal((await call('GET', `/projects/${p.id}`)).status, 404, 'project gone from the hub');

  // plain removal still keeps files
  const [i] = (await call('GET', '/projects')).json.filter((x: any) => x.path === inner);
  assert.equal((await call('DELETE', `/projects/${i.id}`)).status, 200);
  assert.ok(existsSync(inner), 'files stay without ?files=1');
});

test('archive: hidden from lists, restore, delete one, empty the archive', async () => {
  const mk = async (title: string) => (await call('POST', '/sessions', { title, agent: { kind: 'claude-code' } })).json;
  const [a, b, c] = [await mk('arch-a'), await mk('arch-b'), await mk('arch-c')];
  const ids = (list: any[]) => list.map((s) => s.id);

  for (const s of [a, b, c]) assert.equal((await call('POST', `/sessions/${s.id}/archive`)).json.archivedAt !== null, true);
  const all = ids((await call('GET', '/sessions?all=1')).json);
  assert.ok(![a.id, b.id, c.id].some((id) => all.includes(id)), 'archived sessions leave the history');
  assert.deepEqual(ids((await call('GET', '/sessions?archived=1')).json).filter((id: string) => [a.id, b.id, c.id].includes(id)).sort(), [a.id, b.id, c.id].sort());

  const r = await call('POST', `/sessions/${a.id}/restore`);
  assert.equal(r.json.archivedAt, null);
  assert.ok(ids((await call('GET', '/sessions?all=1')).json).includes(a.id), 'restored');

  assert.equal((await call('DELETE', `/sessions/${b.id}`)).status, 200);
  const emptied = await call('DELETE', '/sessions/archive');
  assert.equal(emptied.json.deleted, 1, 'only c was left in the archive');
  assert.deepEqual((await call('GET', '/sessions?archived=1')).json, []);
  assert.ok(ids((await call('GET', '/sessions?all=1')).json).includes(a.id), 'active sessions survive emptying the archive');
  assert.equal((await call('POST', '/sessions/missing/archive')).status, 404);
});

test('a new chat folder is named by time and session id, not by the message', async () => {
  const s = (await call('POST', '/sessions', { title: 'Что там в файле секретного', agent: { kind: 'claude-code' } })).json;
  const name = s.cwd.split('/').pop();
  assert.match(name, new RegExp(`^\\d{4}-\\d{2}-\\d{2}_\\d{2}-\\d{2}-\\d{2}_${s.id.replace(/-/g, '').slice(0, 8)}$`));
});
