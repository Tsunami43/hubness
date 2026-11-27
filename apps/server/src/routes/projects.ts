import { basename } from 'node:path';
import { Hono } from 'hono';
import { z } from 'zod';
import { groups } from '../db/index.ts';
import {
  assertRemovable,
  cloneRepo,
  createGroup,
  createRepo,
  deletableFolder,
  deleteFolder,
  deleteGroup,
  discoverRepos,
  groupAgentContext,
  groupDetail,
  groupOr404,
  importProjects,
  listProjects,
  projectDetail,
  projectOr404,
  refreshGroupRepos,
  removeProject,
  setDefaultInstructions,
  setDefaultPermission,
  setSessionInstructions,
  settings,
  updateGroup,
  updateProject,
} from '../catalog.ts';
import { createManifest, manifestResponse } from '../manifest.ts';
import { stopProjectProcesses } from '../runs.ts';
import { stopContainer } from '../environments.ts';
import { deleteGitCredential, ghLogin, listGitCredentials, setGitCredential } from '../git-auth.ts';
import { HttpError } from '../errors.ts';
import { isActive } from '../runtime.ts';
import { parseBody, PermissionModeSchema, shortText, TermSizeSchema } from './http.ts';

/** Projects and groups, hub settings, the project manifest, git logins per host. */
export const projectRoutes = new Hono();

const Import = z.object({ paths: z.array(z.string()).min(1, 'paths must be a non-empty list of folders'), groupId: z.string().optional() });
const Settings = z
  .object({ defaultPermission: PermissionModeSchema.optional(), defaultInstructions: z.string().optional(), sessionInstructions: z.string().optional() })
  .refine((b) => b.defaultPermission !== undefined || b.defaultInstructions !== undefined || b.sessionInstructions !== undefined, 'Nothing to save');
const NewRepo = z.object({ name: shortText().optional(), groupId: z.string().optional() });
const Clone = TermSizeSchema.extend({ url: z.string().max(2000), name: shortText().optional(), groupId: z.string().optional() });
// lengths and folder rules are checked by the catalog with specific messages
const GroupFields = z.object({
  name: z.string().optional(),
  description: z.string().optional(),
  instructions: z.string().optional(),
  folder: z.string().nullish(),
  projectIds: z.array(z.string()).optional(),
});
const ProjectPatch = z.object({ name: z.string().optional(), description: z.string().optional(), instructions: z.string().optional() });
const GitCredential = z.object({ username: z.string(), kind: z.enum(['token', 'password'], { error: 'kind must be token or password' }), secret: z.string() });

// ---------- projects ----------

projectRoutes.get('/projects', (c) => c.json(listProjects()));
/** Repositories in a folder (default: the projects folder) not yet added to the hub: import suggestions. */
projectRoutes.get('/projects/discover', (c) => c.json(discoverRepos(c.req.query('path'))));
projectRoutes.post('/projects/import', async (c) => c.json(importProjects(await parseBody(c, Import)), 201));
projectRoutes.post('/projects/new-repo', async (c) => c.json(await createRepo(await parseBody(c, NewRepo)), 201));
/** Clone by URL is a terminal job (/api/jobs/:id/stream); the project appears after a successful git clone. */
projectRoutes.post('/projects/clone', async (c) => c.json(cloneRepo(await parseBody(c, Clone)), 201));

projectRoutes.get('/projects/:id', async (c) => c.json(await projectDetail(c.req.param('id'))));
projectRoutes.patch('/projects/:id', async (c) => c.json(updateProject(c.req.param('id'), await parseBody(c, ProjectPatch))));

/**
 * Remove a project from the hub; the project's processes and container are stopped. Files stay
 * on disk unless `?files=1&confirm=<folder name>`: then the repository folder is deleted too
 * (irreversible; the folder name guards against an accidental call).
 */
projectRoutes.delete('/projects/:id', async (c) => {
  const id = c.req.param('id');
  const withFiles = c.req.query('files') === '1';
  const p = assertRemovable(id, isActive);
  const dir = withFiles ? deletableFolder(p) : null;
  if (dir && c.req.query('confirm') !== basename(dir)) throw new HttpError(400, 'confirm must be the name of the project folder');
  stopProjectProcesses(id);
  await stopContainer(id);
  if (dir) deleteFolder(dir);
  removeProject(id, isActive);
  return c.json({ ok: true, deleted: dir });
});

// ---------- settings ----------

projectRoutes.get('/settings', (c) => c.json(settings()));
projectRoutes.put('/settings', async (c) => {
  // the projects folder is fixed (~/hub/projects); default permission and instructions are settings
  const b = await parseBody(c, Settings);
  if (b.defaultPermission !== undefined) setDefaultPermission(b.defaultPermission);
  if (b.defaultInstructions !== undefined) setDefaultInstructions(b.defaultInstructions);
  if (b.sessionInstructions !== undefined) setSessionInstructions(b.sessionInstructions);
  return c.json(settings());
});

// ---------- project groups ----------

projectRoutes.get('/groups', (c) => c.json(groups.list()));
projectRoutes.get('/groups/:id', (c) => c.json(groupDetail(c.req.param('id'))));
projectRoutes.post('/groups', async (c) => c.json(createGroup(await parseBody(c, GroupFields)), 201));
projectRoutes.patch('/groups/:id', async (c) => c.json(updateGroup(c.req.param('id'), await parseBody(c, GroupFields))));
projectRoutes.delete('/groups/:id', (c) => {
  deleteGroup(c.req.param('id'), isActive);
  return c.json({ ok: true });
});
/** Aggregated context the agent will receive in a group session (preview). */
projectRoutes.get('/groups/:id/context', async (c) => {
  const g = groupOr404(c.req.param('id'));
  await refreshGroupRepos(g);
  return c.json({ context: groupAgentContext(groupOr404(g.id)) });
});

// ---------- .hub/hub.yaml manifest ----------

projectRoutes.get('/projects/:id/manifest', async (c) => c.json(await manifestResponse(projectOr404(c.req.param('id')))));
projectRoutes.post('/projects/:id/manifest', async (c) => {
  const p = projectOr404(c.req.param('id'));
  createManifest(p);
  return c.json(await manifestResponse(p), 201);
});

// ---------- git logins per host (Settings → Git access) ----------

projectRoutes.get('/git-credentials', (c) => c.json(listGitCredentials()));
projectRoutes.put('/git-credentials/:host', async (c) => c.json(setGitCredential(c.req.param('host'), await parseBody(c, GitCredential))));
projectRoutes.delete('/git-credentials/:host', (c) => c.json(deleteGitCredential(c.req.param('host'))));
projectRoutes.post('/git-credentials/:host/gh-login', async (c) => c.json(ghLogin(c.req.param('host'), await parseBody(c, TermSizeSchema, { lenient: true })), 201));
