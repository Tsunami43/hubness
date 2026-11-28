import { Hono, type Context } from 'hono';
import { AgentConfigSchema, parseBody } from './http.ts';
import { z } from 'zod';
import { HttpError } from '../errors.ts';
import { askAbout } from '../system/ask.ts';
import { hubVersion } from '../version.ts';
import { systemPlatform } from '../system/host.ts';
import { managerPackages, packageManagers } from '../system/packages.ts';
import {
  containerAction,
  containerRemove,
  dockerNetworks,
  dockerVolumes,
  networkRemove,
  volumeRemove,
  containerLogs,
  dockerContainers,
  overview,
  packageInfo,
  unitAction,
  unitLogs,
  units,
} from '../system/monitor.ts';

/** /api/system: server monitoring; /api/ask: “Ask about this”. */
export const system = new Hono();

const Ask = z.object({
  target: z.discriminatedUnion(
    'kind',
    [
      z.object({ kind: z.literal('container'), id: z.string().min(1) }),
      z.object({ kind: z.literal('unit'), id: z.string().min(1), scope: z.enum(['system', 'user']) }),
      z.object({ kind: z.literal('package'), id: z.string().min(1) }),
    ],
    { error: 'target: container | unit | package' },
  ),
  agent: AgentConfigSchema.optional(),
  question: z.string().max(100_000).optional(),
});

const ACTIONS = ['start', 'stop', 'restart'] as const;
type Action = (typeof ACTIONS)[number];
const action = (a: string): Action => {
  if (!ACTIONS.includes(a as Action)) throw new HttpError(400, 'action: start | stop | restart');
  return a as Action;
};
const scope = (s: string): 'system' | 'user' => {
  if (s !== 'system' && s !== 'user') throw new HttpError(400, 'scope: system | user');
  return s;
};

system.get('/platform', (c) => c.json(systemPlatform()));
system.get('/version', async (c) => c.json(await hubVersion(c.req.query('check') === '1')));
// a checkout is updated with git, not by the hub
system.post('/update', () => {
  throw new HttpError(400, 'This hub runs from a source checkout and can\'t update itself: pull and restart it');
});
system.get('/overview', async (c) => c.json(await overview()));

system.get('/containers', async (c) => {
  const list = await dockerContainers();
  return c.json({ available: list !== null, containers: list ?? [] });
});
system.delete('/containers/:name', async (c) => c.json(await containerRemove(c.req.param('name'))));
system.get('/networks', async (c) => {
  const list = await dockerNetworks();
  return c.json({ available: list !== null, networks: list ?? [] });
});
system.delete('/networks/:name', async (c) => c.json(await networkRemove(c.req.param('name'))));
system.get('/volumes', async (c) => {
  const list = await dockerVolumes();
  return c.json({ available: list !== null, volumes: list ?? [] });
});
system.delete('/volumes/:name', async (c) => c.json(await volumeRemove(c.req.param('name'))));
system.post('/containers/:name/:action', async (c) => c.json(await containerAction(c.req.param('name'), action(c.req.param('action')))));
system.get('/containers/:name/logs', async (c) => c.json({ logs: await containerLogs(c.req.param('name'), Number(c.req.query('lines') ?? 200)) }));

system.get('/units', async (c) => c.json(await units()));
system.post('/units/:scope/:name/:action', async (c) =>
  c.json(await unitAction(c.req.param('name'), scope(c.req.param('scope')), action(c.req.param('action')))),
);
system.get('/units/:scope/:name/logs', async (c) =>
  c.json({ logs: await unitLogs(c.req.param('name'), scope(c.req.param('scope')), Number(c.req.query('lines') ?? 200)) }),
);

system.get('/packages', (c) => c.json({ managers: packageManagers() }));
system.get('/packages/:manager', async (c) => c.json(await managerPackages(c.req.param('manager'), c.req.query('refresh') === '1')));
system.get('/package-info/:name', async (c) => c.json({ info: await packageInfo(c.req.param('name')) }));

/** “Ask about this”: an agent session with the object's context attached. */
export async function handleAsk(c: Context) {
  const b = await parseBody(c, Ask);
  const s = await askAbout(b.target, { agent: b.agent, question: b.question });
  return c.json(s, 201);
}
