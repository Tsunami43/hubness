import { Hono } from 'hono';
import { z } from 'zod';
import { projectOr404 } from '../catalog.ts';
import { HttpError } from '../errors.ts';
import * as git from '../git.ts';
import { parseBody } from './http.ts';

/** Git panel of a project: status, diff, branches, log, stashes and operations. */
export const gitRoutes = new Hono();

const Paths = z.array(z.string()).min(1, 'paths are required');
// one body for every operation: each op reads the fields it needs (no body = defaults)
const GitOp = z.object({
  message: z.string().max(100_000).optional(),
  all: z.boolean().optional(),
  paths: z.array(z.string()).optional(),
  branch: z.string().max(500).optional(),
  create: z.boolean().optional(),
  rev: z.string().max(500).optional(),
  mode: z.enum(['soft', 'mixed', 'hard']).optional(),
  force: z.boolean().optional(),
  noFf: z.boolean().optional(),
  untracked: z.boolean().optional(),
  action: z.string().optional(),
});
const StashAction = z.enum(['apply', 'pop', 'drop'], { error: 'action: apply | pop | drop' });

/** Project repository folder (a project = exactly one repository). */
const rootPath = (c: { req: { param(k: string): string } }): string => projectOr404(c.req.param('id')).path;

/** A required field of an op: a 400 naming it, like the body schema does. */
function need<T>(schema: z.ZodType<T>, value: unknown, field: string): T {
  const r = schema.safeParse(value);
  if (!r.success) throw new HttpError(400, `${field}: ${r.error.issues[0]?.message ?? 'invalid'}`);
  return r.data;
}

gitRoutes.get('/projects/:id/git/status', async (c) => c.json(await git.status(rootPath(c))));

gitRoutes.get('/projects/:id/git/diff', async (c) => {
  const diff = await git.diff(rootPath(c), {
    path: c.req.query('path') || undefined,
    staged: c.req.query('staged') === '1',
    untracked: c.req.query('untracked') === '1',
  });
  return c.json({ diff });
});

gitRoutes.get('/projects/:id/git/branches', async (c) => c.json(await git.branches(rootPath(c))));

gitRoutes.get('/projects/:id/git/log', async (c) =>
  c.json(await git.log(rootPath(c), Math.min(2000, Number(c.req.query('limit') ?? 150) || 150), c.req.query('all') !== '0')),
);

gitRoutes.get('/projects/:id/git/show', async (c) => c.json({ diff: await git.show(rootPath(c), c.req.query('rev') ?? '') }));

gitRoutes.get('/projects/:id/git/stashes', async (c) => c.json(await git.stashes(rootPath(c))));

gitRoutes.post('/projects/:id/git/:op', async (c) => {
  const cwd = rootPath(c);
  const op = c.req.param('op');
  const b = await parseBody(c, GitOp, { lenient: true });
  switch (op) {
    case 'fetch':
      return c.json(await git.ops.fetch(cwd));
    case 'pull':
      return c.json(await git.ops.pull(cwd));
    case 'push':
      return c.json(await git.ops.push(cwd));
    case 'commit':
      return c.json(await git.ops.commit(cwd, b.message ?? '', b.all ?? true));
    case 'stage':
      return c.json(await git.ops.stage(cwd, need(Paths, b.paths, 'paths')));
    case 'unstage':
      return c.json(await git.ops.unstage(cwd, need(Paths, b.paths, 'paths')));
    case 'checkout':
      return c.json(await git.ops.checkout(cwd, need(z.string().trim().min(1, 'Branch is required'), b.branch ?? '', 'branch'), Boolean(b.create)));
    case 'branch-at':
      return c.json(await git.ops.branchAt(cwd, b.branch?.trim() ?? '', b.rev ?? ''));
    case 'delete-branch':
      return c.json(await git.ops.deleteBranch(cwd, b.branch ?? '', Boolean(b.force)));
    case 'merge':
      return c.json(await git.ops.merge(cwd, b.branch ?? '', Boolean(b.noFf)));
    case 'rebase':
      return c.json(await git.ops.rebase(cwd, b.branch ?? ''));
    case 'reset':
      return c.json(await git.ops.reset(cwd, b.rev ?? '', b.mode ?? 'mixed'));
    case 'cherry-pick':
      return c.json(await git.ops.cherryPick(cwd, b.rev ?? ''));
    case 'revert':
      return c.json(await git.ops.revert(cwd, b.rev ?? ''));
    case 'continue':
    case 'abort':
      return c.json(await git.ops.proceed(cwd, op));
    case 'discard':
      return c.json(await git.ops.discard(cwd, need(Paths, b.paths, 'paths')));
    case 'stash-push':
      return c.json(await git.ops.stashPush(cwd, b.message ?? '', b.untracked ?? true));
    case 'stash':
      return c.json(await git.ops.stash(cwd, need(StashAction, b.action, 'action'), b.rev ?? ''));
    default:
      throw new HttpError(404, 'Unknown git operation');
  }
});
