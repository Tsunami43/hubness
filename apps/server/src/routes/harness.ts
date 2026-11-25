import { Hono } from 'hono';
import { streamSSE } from 'hono/streaming';
import type { HarnessAction, JobStreamMsg } from '@hubness/shared';
import { HttpError } from '../errors.ts';
import { agentsList, harnessAll, harnessOne, invalidateHarness } from '../harness.ts';
import { agentOr404 } from '../agents/registry.ts';
import { z } from 'zod';
import { getJob, killJob, startJob, subscribeJob, writeJob } from '../terminal.ts';
import { parseBody, pumpSSE, TermSizeSchema } from './http.ts';
import { extensionsOf, projectDirOf, readListedFile, readRules, saveRules } from '../extensions.ts';
import { mcpAdd, mcpConnect, mcpDelete, mcpDisconnect, mcpMatrix } from '../mcp-servers.ts';
import { mcpCatalog } from '../mcp-catalog.ts';
import { AGENTS } from '../agents/registry.ts';
import { skillConnect, skillContent, skillCreate, skillDelete, skillDisconnect, skillImport, skillImportPreview, skillsMatrix } from '../skills.ts';

/**
 * /api/harness: harness info and actions; /api/jobs: terminal
 * jobs. Only actions from the harness's list are run (no arbitrary commands).
 */
export const harness = new Hono();
export const jobs = new Hono();

const DefaultModel = z.object({ model: z.string().max(200).nullish() });
const JobInput = z.object({ data: z.string().max(4096, 'data: at most 4096 characters') });

const McpBody = z.object({
  name: z.string().max(64),
  transport: z.enum(['stdio', 'http', 'sse']),
  command: z.string().max(1000).nullish(),
  args: z.array(z.string().max(2000)).max(100).nullish(),
  env: z.record(z.string(), z.string()).nullish(),
  url: z.string().max(2000).nullish(),
  headers: z.record(z.string(), z.string()).nullish(),
  agents: z.array(z.string().max(64)).max(20),
});
const RulesBody = z.object({ content: z.string() });
const Agents = z.array(z.string().max(64)).max(20);
const SkillBody = z.object({
  name: z.string().max(64),
  description: z.string().max(4000),
  body: z.string(),
  agents: Agents,
  replace: z.boolean().nullish(),
});
const ImportPreviewBody = z.object({ url: z.string().max(2000) });
const ImportBody = z.object({ id: z.string().max(64), skills: z.array(z.string().max(64)).max(200), agents: Agents, replace: z.boolean().nullish() });

harness.get('/', async (c) => c.json(await harnessAll(c.req.query('refresh') === '1')));
harness.get('/agents', async (c) => c.json(await agentsList()));

// ---------- MCP servers in the agents' own configs (before /:kind) ----------

const project = (c: { req: { query(k: string): string | undefined } }) => projectDirOf(c.req.query('project'));

harness.get('/mcp', (c) => c.json(mcpMatrix(project(c))));
harness.get('/mcp/catalog', async (c) => c.json(await mcpCatalog(c.req.query('q'), c.req.query('cursor'))));
harness.post('/mcp', async (c) => {
  const b = await parseBody(c, McpBody);
  const dir = project(c);
  return c.json(
    mcpAdd(
      { name: b.name, transport: b.transport, command: b.command ?? undefined, args: b.args ?? undefined, env: b.env ?? undefined, url: b.url ?? undefined, headers: b.headers ?? undefined, agents: b.agents },
      dir,
    ),
  );
});
harness.put('/mcp/:name/agents/:kind', (c) => c.json(mcpConnect(c.req.param('name'), c.req.param('kind'), project(c))));
harness.delete('/mcp/:name/agents/:kind', (c) => c.json(mcpDisconnect(c.req.param('name'), c.req.param('kind'), project(c))));
harness.delete('/mcp/:name', (c) => c.json(mcpDelete(c.req.param('name'), project(c))));

// ---------- skills across the agents (before /:kind) ----------

harness.get('/skills', (c) => c.json(skillsMatrix(project(c))));
harness.post('/skills', async (c) => {
  const b = await parseBody(c, SkillBody);
  return c.json(skillCreate({ ...b, replace: b.replace ?? undefined }, project(c)));
});
harness.post('/skills/import/preview', async (c) => c.json(await skillImportPreview((await parseBody(c, ImportPreviewBody)).url)));
harness.post('/skills/import', async (c) => {
  const b = await parseBody(c, ImportBody);
  return c.json(skillImport({ ...b, replace: b.replace ?? undefined }, project(c)));
});
harness.get('/skills/:name', (c) => c.json(skillContent(c.req.param('name'), c.req.query('agent') || undefined, project(c))));
harness.delete('/skills/:name', (c) => c.json(skillDelete(c.req.param('name'), project(c))));
harness.put('/skills/:name/agents/:kind', (c) => c.json(skillConnect(c.req.param('name'), c.req.param('kind'), project(c))));
harness.delete('/skills/:name/agents/:kind', (c) => c.json(skillDisconnect(c.req.param('name'), c.req.param('kind'), project(c))));

// ---------- skills, rules, subagents, commands ----------

/** Every agent's extensions at once (the cross-agent Rules / Subagents / Commands / Plugins pages). */
harness.get('/extensions', (c) => {
  const dir = project(c);
  return c.json({ agents: AGENTS.map((a) => extensionsOf(a.kind, dir)) });
});
harness.get('/:kind/extensions', (c) => c.json(extensionsOf(agentOr404(c.req.param('kind')).kind, project(c))));
harness.get('/:kind/file', (c) => c.json(readListedFile(agentOr404(c.req.param('kind')).kind, project(c), c.req.query('path'))));
harness.get('/:kind/rules', (c) => c.json(readRules(agentOr404(c.req.param('kind')).kind, project(c), c.req.query('path'))));
harness.put('/:kind/rules', async (c) => {
  const kind = agentOr404(c.req.param('kind')).kind;
  const b = await parseBody(c, RulesBody);
  return c.json(saveRules(kind, project(c), c.req.query('path'), b.content));
});
/** One harness (a tab of the Harness page); ?refresh=1 re-checks versions online. */
harness.get('/:kind', async (c) => c.json(await harnessOne(c.req.param('kind'), c.req.query('refresh') === '1')));

harness.post('/:kind/actions/:action', async (c) => {
  const agent = agentOr404(c.req.param('kind'));
  const id = c.req.param('action') as HarnessAction['id'];
  const b = await parseBody(c, TermSizeSchema, { lenient: true });
  const { info } = await harnessOne(agent.kind);
  const act = info.actions.find((a) => a.id === id);
  if (!act) throw new HttpError(404, 'Action is not available');
  if (!act.runnable) throw new HttpError(403, 'This command needs root — run it in a terminal yourself');

  const job = startJob({
    title: `${agent.label} · ${id}`,
    command: act.command,
    cols: b.cols,
    rows: b.rows,
    onExit: () => invalidateHarness(agent.kind),
  });
  return c.json(job, 201);
});

// ---------- provider credentials (accountMode: providers) ----------

const ID_RE = /^[\w.:-]{1,120}$/;
harness.post('/:kind/accounts/:provider/:account/:op', async (c) => {
  const agent = agentOr404(c.req.param('kind'));
  const { provider, account, op } = c.req.param();
  if (!agent.accountOp) throw new HttpError(404, 'This agent has no provider accounts');
  if (!ID_RE.test(provider) || !ID_RE.test(account)) throw new HttpError(400, 'Bad account id');
  if (op !== 'switch' && op !== 'logout') throw new HttpError(400, 'op: switch | logout');
  try {
    return c.json({ ok: true, output: await agent.accountOp(provider, account, op) });
  } finally {
    invalidateHarness(agent.kind);
  }
});

harness.put('/:kind/default-model', async (c) => {
  const agent = agentOr404(c.req.param('kind'));
  if (!agent.setDefaultModel) throw new HttpError(404, 'This agent has no default model setting');
  const b = await parseBody(c, DefaultModel);
  agent.setDefaultModel(b.model?.trim() || null);
  invalidateHarness(agent.kind);
  return c.json({ ok: true });
});

// ---------- terminal jobs ----------

jobs.get('/:id/stream', (c) => {
  const id = c.req.param('id');
  const j = getJob(id);
  if (!j) throw new HttpError(404, 'Job not found');
  return streamSSE(c, (stream) =>
    pumpSSE<JobStreamMsg>(stream, (push) => subscribeJob(id, push), {
      first: { type: 'snapshot', job: j.info, output: j.output },
      done: (m) => m.type === 'exit',
    }),
  );
});

jobs.post('/:id/input', async (c) => {
  const b = await parseBody(c, JobInput);
  if (!writeJob(c.req.param('id'), b.data)) throw new HttpError(409, 'The command is not running');
  return c.json({ ok: true });
});

jobs.post('/:id/kill', (c) => c.json({ ok: killJob(c.req.param('id')) }));
