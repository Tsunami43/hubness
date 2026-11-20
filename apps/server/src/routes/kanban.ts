import { Hono } from 'hono';
import { z } from 'zod';
import { bus } from '../bus.ts';
import { cards, sessions } from '../db/index.ts';
import { projectOr404 } from '../catalog.ts';
import { HttpError } from '../errors.ts';
import { sendMessage } from '../runtime.ts';
import { AgentConfigSchema, parseBody, validateAgent } from './http.ts';

/** Kanban cards of a project (API without UI for now) and starting an agent from a card. */
export const kanbanRoutes = new Hono();

const Column = z.enum(['todo', 'doing', 'review', 'done'], { error: 'Unknown column' });
const CreateCard = z.object({ title: z.string().trim().min(1, 'Title is required').max(500), description: z.string().max(100_000).default(''), column: Column.optional() });
const PatchCard = z.object({ title: z.string().max(500).optional(), description: z.string().max(100_000).optional(), column: Column.optional(), position: z.number().optional() });
const StartCard = z.object({ agent: AgentConfigSchema });

kanbanRoutes.get('/projects/:id/cards', (c) => c.json(cards.list(c.req.param('id'))));

kanbanRoutes.post('/projects/:id/cards', async (c) => {
  const p = projectOr404(c.req.param('id'));
  const b = await parseBody(c, CreateCard);
  return c.json(cards.create(p.id, { title: b.title, description: b.description, column: b.column ?? 'todo' }), 201);
});

kanbanRoutes.patch('/cards/:id', async (c) => {
  const b = await parseBody(c, PatchCard);
  const card = cards.update(c.req.param('id'), { title: b.title?.trim() || undefined, description: b.description, column: b.column, position: b.position });
  if (!card) throw new HttpError(404, 'Card not found');
  return c.json(card);
});

kanbanRoutes.delete('/cards/:id', (c) => {
  cards.delete(c.req.param('id'));
  return c.json({ ok: true });
});

/** Start an agent from a card: new session, prompt = title + description. */
kanbanRoutes.post('/cards/:id/start', async (c) => {
  const card = cards.get(c.req.param('id'));
  if (!card) throw new HttpError(404, 'Card not found');
  projectOr404(card.projectId);
  const b = await parseBody(c, StartCard);
  const s = sessions.create({ projectId: card.projectId, cardId: card.id, title: card.title, agent: validateAgent(b.agent) });
  cards.update(card.id, { sessionId: s.id, column: 'doing' });
  bus.emitGlobal({ type: 'session', session: s });
  const prompt = card.description.trim() ? `${card.title}\n\n${card.description}` : card.title;
  return c.json(sendMessage(s.id, prompt), 201);
});
