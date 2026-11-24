import './setup.ts';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { db, groups, projects, sessions } from '../src/db/index.ts';
import { migrateLegacyGroups } from '../src/db/migrations.ts';

const agent = { kind: 'claude-code' as const, permissionMode: 'ask' as const };

test('a project is one repository; a group references projects without copying them', () => {
  const auth = projects.create('auth-service', '/repos/auth');
  const billing = projects.create('billing-service', '/repos/billing');
  const before = projects.list().length;
  const g = groups.create({ name: 'Commerce Platform', projectIds: [auth.id, billing.id] });
  assert.deepEqual(g.projectIds, [auth.id, billing.id]);
  assert.equal(projects.list().length, before, 'adding to a group must not create project records');
  assert.deepEqual(projects.get(auth.id)!.groupIds, [g.id]);
});

test('a project can belong to several groups; deleting a group keeps projects', () => {
  const a = projects.create('frontend', '/repos/frontend');
  const b = projects.create('infra', '/repos/infra');
  const c = projects.create('orders', '/repos/orders');
  const g1 = groups.create({ name: 'Shop', projectIds: [a.id, b.id] });
  const g2 = groups.create({ name: 'Platform', projectIds: [b.id, c.id] });
  assert.deepEqual(new Set(projects.get(b.id)!.groupIds), new Set([g1.id, g2.id]));
  groups.delete(g1.id);
  assert.ok(projects.get(a.id), 'project survives group deletion');
  assert.deepEqual(projects.get(b.id)!.groupIds, [g2.id]);
});

test('deleting a project removes it from groups; member order is kept', () => {
  const a = projects.create('x', '/repos/x');
  const b = projects.create('y', '/repos/y');
  const c = projects.create('z', '/repos/z');
  const g = groups.create({ name: 'XYZ', projectIds: [c.id, a.id, b.id] });
  assert.deepEqual(g.projectIds, [c.id, a.id, b.id]);
  projects.delete(a.id);
  assert.deepEqual(groups.get(g.id)!.projectIds, [c.id, b.id]);
});

test('a session belongs to exactly one scope', () => {
  const p = projects.create('svc', '/repos/svc');
  const q = projects.create('svc2', '/repos/svc2');
  const g = groups.create({ name: 'G', projectIds: [p.id, q.id] });
  assert.throws(() => sessions.create({ projectId: p.id, groupId: g.id, title: 't', agent }));
  const gs = sessions.create({ projectId: null, groupId: g.id, title: 'group task', agent });
  assert.equal(gs.groupId, g.id);
  assert.equal(gs.projectId, null);
  assert.deepEqual(sessions.list({ groupId: g.id }).map((s) => s.id), [gs.id]);
  assert.ok(!sessions.list({ chats: true }).some((s) => s.id === gs.id), 'group sessions are not plain chats');
  groups.delete(g.id);
  assert.equal(sessions.get(gs.id), undefined, 'group sessions go with the group');
});

test('membership is transactional: an unknown project rolls back the whole group', () => {
  const a = projects.create('t1', '/repos/t1');
  const count = groups.list().length;
  assert.throws(() => groups.create({ name: 'Broken', projectIds: [a.id, 'nope'] }));
  assert.equal(groups.list().length, count);
});

test('legacy groups (projects.kind = group) migrate to project_groups', () => {
  const a = projects.create('legacy-a', '/repos/legacy-a');
  const b = projects.create('legacy-b', '/repos/legacy-b');
  const t = new Date().toISOString();
  db.prepare("INSERT INTO projects (id, name, path, kind, created_at, updated_at) VALUES ('old-g', 'Old group', '/data/groups/old', 'group', ?, ?)").run(t, t);
  db.prepare("INSERT INTO project_roots (id, project_id, path, name, position) VALUES ('r1', 'old-g', '/repos/legacy-a', 'a', 0), ('r2', 'old-g', '/repos/legacy-b', 'b', 1)").run();
  db.prepare(
    "INSERT INTO sessions (id, project_id, title, agent, status, created_at, updated_at) VALUES ('old-s', 'old-g', 'old', ?, 'idle', ?, ?)",
  ).run(JSON.stringify(agent), t, t);

  migrateLegacyGroups(db);

  const g = groups.get('old-g');
  assert.ok(g, 'group keeps its id');
  assert.equal(g.name, 'Old group');
  assert.deepEqual(g.projectIds, [a.id, b.id]);
  assert.equal(projects.get('old-g'), undefined, 'no project row left for the group');
  const s = sessions.get('old-s')!;
  assert.equal(s.groupId, 'old-g');
  assert.equal(s.projectId, null);
  migrateLegacyGroups(db); // idempotent
  assert.equal(groups.list().filter((x) => x.id === 'old-g').length, 1);
});
