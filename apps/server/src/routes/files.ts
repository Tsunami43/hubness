import { Hono } from 'hono';
import { streamSSE } from 'hono/streaming';
import { z } from 'zod';
import type { FileChangeMsg } from '@hubness/shared';
import { projectOr404 } from '../catalog.ts';
import { HttpError } from '../errors.ts';
import { browse, createEntry, deleteEntry, listFiles, listTree, readFileSafe, renameEntry, writeFileSafe } from '../fsops.ts';
import { searchFiles } from '../search.ts';
import { watchFolder } from '../watch.ts';
import { parseBody, pumpSSE } from './http.ts';

/** Project files: tree, read / write, create / rename / delete, search, live changes; server folder browsing. */
export const fileRoutes = new Hono();

const PathBody = z.object({ path: z.string().min(1, 'path is required') });
const Create = PathBody.extend({ dir: z.boolean().optional() });
const Rename = z.object({ from: z.string().min(1), to: z.string().min(1) });
const Write = PathBody.extend({ content: z.string(), baseHash: z.string().optional() });

/** Project repository folder (a project = exactly one repository). */
const rootPath = (c: { req: { param(k: string): string } }): string => projectOr404(c.req.param('id')).path;

fileRoutes.get('/fs/browse', (c) => c.json(browse(c.req.query('path'))));

fileRoutes.get('/projects/:id/tree', (c) => c.json(listTree(rootPath(c), c.req.query('path') ?? '')));
fileRoutes.get('/projects/:id/files', (c) => c.json({ files: listFiles(rootPath(c)) }));

fileRoutes.get('/projects/:id/file', (c) => {
  const rel = c.req.query('path');
  if (!rel) throw new HttpError(400, 'path is required');
  return c.json(readFileSafe(rootPath(c), rel));
});

fileRoutes.put('/projects/:id/file', async (c) => {
  const b = await parseBody(c, Write);
  return c.json(writeFileSafe(rootPath(c), b.path, b.content, b.baseHash || undefined));
});

fileRoutes.get('/projects/:id/search', async (c) => {
  const q = c.req.query();
  return c.json(
    await searchFiles(rootPath(c), { q: q.q ?? '', regex: q.regex === '1', caseSensitive: q.case === '1', word: q.word === '1', include: q.include, exclude: q.exclude }),
  );
});

fileRoutes.post('/projects/:id/fs/create', async (c) => {
  const b = await parseBody(c, Create);
  return c.json(createEntry(rootPath(c), b.path, Boolean(b.dir)), 201);
});
fileRoutes.post('/projects/:id/fs/rename', async (c) => {
  const b = await parseBody(c, Rename);
  return c.json(renameEntry(rootPath(c), b.from, b.to));
});
fileRoutes.post('/projects/:id/fs/delete', async (c) => {
  const b = await parseBody(c, PathBody);
  deleteEntry(rootPath(c), b.path);
  return c.json({ ok: true });
});

/**
 * Live file changes of the given projects (`?projects=id1,id2`): one stream per workbench, so
 * a group with many repositories does not use up the browser's connection limit.
 */
fileRoutes.get('/watch', (c) => {
  const ids = [...new Set((c.req.query('projects') ?? '').split(',').filter(Boolean))].slice(0, 50);
  const list = ids.map((id) => projectOr404(id));
  return streamSSE(c, (stream) =>
    pumpSSE<FileChangeMsg>(stream, (push) => {
      const offs = list.map((p) => watchFolder(p.path, (m) => push({ projectId: p.id, ...m })));
      return () => offs.forEach((off) => off());
    }),
  );
});
