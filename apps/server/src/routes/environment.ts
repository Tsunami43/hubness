import { Hono } from 'hono';
import { z } from 'zod';
import { groupOr404, groupProjects, projectOr404 } from '../catalog.ts';
import { createRun, deleteRun, listRuns, listShells, openShell, previewPortFor, startRun, stopProjectProcesses, stopRun, suggestRuns, updateRun } from '../runs.ts';
import { environmentStatus, runtimeOf, setRuntime, stopContainer } from '../environments.ts';
import { deleteSecret, keySource, listSecrets, setSecret, unreadableSecrets } from '../secrets.ts';
import { groupWorkspace } from '../workspaces/index.ts';
import { parseBody, TermSizeSchema } from './http.ts';

/** Run commands, shells, previews, the execution environment and secrets of a project (or group shells). */
export const environmentRoutes = new Hono();

// name/command limits and the port range are checked by runs.ts with specific messages
const RunFields = z.object({
  name: z.string().optional(),
  command: z.string().optional(),
  port: z.union([z.number(), z.string(), z.null()]).optional(),
  primary: z.boolean().optional(),
  docker: z.boolean().optional(),
});
const Shell = TermSizeSchema.extend({ fresh: z.boolean().optional() });
const Preview = z.object({ port: z.coerce.number().int().min(1).max(65535) });
const Runtime = z.object({
  environment: z.enum(['local', 'docker'], { error: 'environment must be local or docker' }).optional(),
  image: z.string().max(255).optional(),
  sandbox: z.boolean().optional(),
  shareSecretsWithAgents: z.boolean().optional(),
});
const Secret = z.object({ value: z.string().max(64_000) });

const project = (c: { req: { param(k: string): string } }) => projectOr404(c.req.param('id'));
const groupScope = (id: string) => {
  const g = groupOr404(id);
  const list = groupProjects(g);
  return { kind: 'group' as const, group: g, projects: list, cwd: groupWorkspace(g.id, list).path };
};

// ---------- Run ----------

environmentRoutes.get('/projects/:id/runs', (c) => c.json(listRuns(project(c))));
environmentRoutes.get('/projects/:id/runs/suggest', (c) => c.json(suggestRuns(project(c))));
environmentRoutes.post('/projects/:id/runs', async (c) => c.json(createRun(project(c), await parseBody(c, RunFields)), 201));
environmentRoutes.patch('/projects/:id/runs/:rid', async (c) => c.json(updateRun(project(c), c.req.param('rid'), await parseBody(c, RunFields))));
environmentRoutes.delete('/projects/:id/runs/:rid', (c) => {
  deleteRun(project(c), c.req.param('rid'));
  return c.json({ ok: true });
});
environmentRoutes.post('/projects/:id/runs/:rid/start', async (c) =>
  c.json(await startRun(project(c), c.req.param('rid'), await parseBody(c, TermSizeSchema, { lenient: true }))),
);
environmentRoutes.post('/projects/:id/runs/:rid/stop', (c) => c.json(stopRun(project(c), c.req.param('rid'))));

// ---------- Shell ----------

/** Project shell: an already open one is returned (reconnect); fresh opens a new one. */
environmentRoutes.post('/projects/:id/shell', async (c) => {
  const { fresh, ...size } = await parseBody(c, Shell, { lenient: true });
  return c.json(openShell({ kind: 'project', project: project(c) }, size, fresh === true));
});
environmentRoutes.post('/groups/:id/shell', async (c) => {
  const { fresh, ...size } = await parseBody(c, Shell, { lenient: true });
  return c.json(openShell(groupScope(c.req.param('id')), size, fresh === true));
});
environmentRoutes.get('/projects/:id/shells', (c) => c.json(listShells({ kind: 'project', project: project(c) })));
environmentRoutes.get('/groups/:id/shells', (c) => c.json(listShells(groupScope(c.req.param('id')))));

/** Preview of a port something is already listening on (Shell, docker compose…). */
environmentRoutes.post('/projects/:id/preview', async (c) => {
  const b = await parseBody(c, Preview);
  return c.json({ previewPort: await previewPortFor(project(c), b.port) });
});

// ---------- environment ----------

environmentRoutes.get('/projects/:id/environment', async (c) => c.json(await environmentStatus(project(c))));
environmentRoutes.put('/projects/:id/environment', async (c) => {
  const p = project(c);
  const before = runtimeOf(p);
  const after = setRuntime(p, await parseBody(c, Runtime));
  // environment changed: running processes use the old one, so stop them
  if (before.environment !== after.environment || before.image !== after.image || before.sandbox !== after.sandbox) {
    stopProjectProcesses(p.id);
    if (before.environment === 'docker') await stopContainer(p.id);
  }
  return c.json(await environmentStatus(p));
});
environmentRoutes.post('/projects/:id/environment/container/remove', async (c) => {
  const p = project(c);
  stopProjectProcesses(p.id);
  await stopContainer(p.id);
  return c.json(await environmentStatus(p));
});

// ---------- secrets ----------

environmentRoutes.get('/projects/:id/secrets', (c) => {
  const p = project(c);
  return c.json({ secrets: listSecrets(p.id), unreadable: unreadableSecrets(p.id), keySource: keySource() });
});
environmentRoutes.put('/projects/:id/secrets/:name', async (c) => {
  const p = project(c);
  const b = await parseBody(c, Secret);
  return c.json(setSecret(p.id, c.req.param('name'), b.value));
});
environmentRoutes.delete('/projects/:id/secrets/:name', (c) => c.json(deleteSecret(project(c).id, c.req.param('name'))));
