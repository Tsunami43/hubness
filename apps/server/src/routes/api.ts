import { Hono } from 'hono';
import { agentsList } from '../harness.ts';
import { agentOr404 } from '../agents/registry.ts';
import { environmentRoutes } from './environment.ts';
import { fileRoutes } from './files.ts';
import { gitRoutes } from './git.ts';
import { kanbanRoutes } from './kanban.ts';
import { projectRoutes } from './projects.ts';
import { sessionRoutes } from './sessions.ts';
import { voiceRoutes } from './voice.ts';

/** /api: the hub REST and SSE API, one router per domain. */
export const api = new Hono();

api.get('/health', (c) => c.json({ ok: true }));
api.get('/agents', async (c) => c.json(await agentsList()));
api.get('/agents/:kind/models', async (c) => c.json(await agentOr404(c.req.param('kind')).models()));

for (const r of [projectRoutes, environmentRoutes, fileRoutes, gitRoutes, sessionRoutes, kanbanRoutes, voiceRoutes]) api.route('/', r);
