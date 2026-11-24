import { makeRepo, tmp } from './setup.ts';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, lstatSync, mkdirSync, readlinkSync } from 'node:fs';
import { join } from 'node:path';
import {
  createGroup,
  deleteGroup,
  groupAgentContext,
  groupDetail,
  createRepo,
  discoverRepos,
  importProjects,
  listProjects,
  projectDetail,
  removeProject,
  scanRepos,
  updateGroup,
  updateProject,
} from '../src/catalog.ts';
import { linkNames } from '../src/workspaces/local.ts';
import { HttpError } from '../src/errors.ts';
import { db, projects, sessions } from '../src/db/index.ts';
import { migrate, pruneAutoImported } from '../src/db/migrations.ts';

const root = join(tmp, 'hub', 'projects'); // the fixed projects folder (HUBNESS_HOME/projects)
const auth = makeRepo(join(root, 'auth-service'), 'git@github.com:acme/auth-service.git');
const billing = makeRepo(join(root, 'commerce', 'billing-service'), 'https://gitlab.com/acme/billing-service.git');
const notify = makeRepo(join(root, 'commerce', 'notification-service'));
mkdirSync(join(root, 'node_modules', 'pkg', '.git'), { recursive: true }); // internal folders are not scanned
mkdirSync(join(root, 'notes'), { recursive: true });

const status = (fn: () => unknown) => {
  try {
    fn();
  } catch (e) {
    return e instanceof HttpError ? e.status : -1;
  }
  return 0;
};

test('scanRepos finds repositories, skips node_modules and does not descend into repos', () => {
  assert.deepEqual(scanRepos(root), [auth, billing, notify].sort());
});

const normal = () => listProjects().filter((p) => p.kind === 'normal');

test('migration removes auto-imported projects that nothing refers to', () => {
  const stray = projects.create('stray', auth, 'normal');
  pruneAutoImported(db);
  assert.equal(projects.get(stray.id), undefined);
  projects.create('stray2', auth, 'normal');
  migrate(db); // already applied on start: runs once, the second start deletes nothing
  assert.equal(normal().length, 1);
  projects.delete(normal()[0]!.id);
});

test('projects folder is storage: nothing is imported until the user picks it', () => {
  assert.equal(normal().length, 0);
  assert.deepEqual(discoverRepos().repos.map((r) => r.path), [auth, billing, notify].sort());
  assert.equal(status(() => importProjects({ paths: [join(root, 'notes')] })), 400, 'not a git repository');
  importProjects({ paths: [auth, billing, notify] });
  importProjects({ paths: [auth] });
  assert.equal(normal().length, 3, 'import does not duplicate');
  assert.equal(discoverRepos().repos.length, 0);
});

const byPath = (path: string) => listProjects().find((p) => p.path === path)!;

test('project detail reads provider, remote and default branch from git', async () => {
  const d = await projectDetail(byPath(auth).id);
  assert.equal(d.repo.provider, 'github');
  assert.equal(d.repo.links?.repo, 'https://github.com/acme/auth-service');
  assert.equal(d.repo.branch, 'main');
  assert.equal(d.workspace.strategy, 'single');
  assert.equal(d.workspace.path, auth);
  assert.equal((await projectDetail(byPath(notify).id)).repo.provider, null, 'no remote — no provider');
});

test('updateProject validates input', () => {
  const id = byPath(auth).id;
  assert.equal(status(() => updateProject(id, { name: '  ' })), 400);
  assert.equal(status(() => updateProject(id, { description: 42 })), 400);
  const p = updateProject(id, { description: 'Issues and refreshes tokens', instructions: 'Never log tokens.' });
  assert.equal(p.description, 'Issues and refreshes tokens');
});

test('group validation: name, known projects, absolute folder', () => {
  const a = byPath(auth).id;
  assert.equal(status(() => createGroup({ name: '', projectIds: [a, byPath(billing).id] })), 400);
  assert.equal(status(() => createGroup({ name: 'G', projectIds: [a, 'missing'] })), 400);
  assert.equal(status(() => createGroup({ name: 'G', folder: 'relative/path', projectIds: [] })), 400);
  assert.equal(status(() => createGroup({ name: 'G', projectIds: 'nope' })), 400);
});

test('group workspace: symlinks to member repositories, updated with membership, removed with the group', () => {
  const g = createGroup({
    name: 'Commerce Platform',
    description: 'Online shop backend',
    instructions: 'Keep API contracts backwards compatible.',
    projectIds: [byPath(auth).id, byPath(billing).id],
  });
  assert.equal(g.workspace.strategy, 'multi');
  assert.equal(g.workspace.status, 'ready');
  const link = join(g.workspace.path, 'auth-service');
  assert.ok(lstatSync(link).isSymbolicLink());
  assert.equal(readlinkSync(link), auth);

  const updated = updateGroup(g.id, { projectIds: [byPath(billing).id, byPath(notify).id] });
  assert.ok(!existsSync(join(updated.workspace.path, 'auth-service')), 'removed member link is gone');
  assert.ok(existsSync(join(updated.workspace.path, 'notification-service')));
  assert.deepEqual(groupDetail(g.id).projects.map((p) => p.name), ['billing-service', 'notification-service']);

  deleteGroup(g.id, () => false);
  assert.ok(!existsSync(updated.workspace.path), 'group workspace folder removed');
  assert.ok(existsSync(billing) && existsSync(notify), 'repositories untouched');
});

test('group context lists every member repository with paths and instructions', async () => {
  await projectDetail(byPath(billing).id); // refresh the remote cache
  const g = createGroup({ name: 'Commerce', instructions: 'Use conventional commits.', projectIds: [byPath(auth).id, byPath(billing).id] });
  const ctx = groupAgentContext(g);
  assert.match(ctx, /"Commerce" — a group of 2 repositories/);
  assert.ok(ctx.includes(auth) && ctx.includes(billing));
  assert.match(ctx, /Use conventional commits\./);
  assert.match(ctx, /Never log tokens\./, 'project instructions are included');
  assert.match(ctx, /billing-service\.git \(GitLab\)/);
  sessions.create({ projectId: null, groupId: g.id, title: 'busy', agent: { kind: 'opencode' } });
  assert.equal(status(() => deleteGroup(g.id, () => true)), 409, 'cannot delete while an agent works');
});

test('linkNames disambiguates folders with the same name', () => {
  assert.deepEqual(linkNames(['/a/api', '/b/api', '/c/web', '/d/api']), ['api', 'api-2', 'web', 'api-3']);
});

test('a group can store its new projects in its own folder', async () => {
  const folder = join(tmp, 'work');
  const g = createGroup({ name: 'Work', folder, projectIds: [] });
  assert.equal(g.folder, folder);
  assert.equal(g.projects.length, 0, 'groups may start empty');
  const p = await createRepo({ name: 'new api', groupId: g.id });
  assert.equal(p.path, join(folder, 'new-api'));
  assert.ok(existsSync(join(p.path, '.git')));
  assert.deepEqual(groupDetail(g.id).projectIds, [p.id], 'created inside the group → member of it');
  const loose = await createRepo({ name: 'loose' });
  assert.equal(loose.path, join(root, 'loose'), 'outside a group → projects folder');
  assert.equal(updateGroup(g.id, { folder: '' }).folder, null);
  await assert.rejects(createRepo({ name: 'loose' }), (e: HttpError) => e.status === 409);

  removeProject(p.id, () => false);
  assert.deepEqual(groupDetail(g.id).projectIds, [], 'removed project leaves its groups');
  assert.ok(existsSync(p.path), 'files stay on disk');
  deleteGroup(g.id, () => false);
});
