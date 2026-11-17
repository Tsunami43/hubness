import type { Context } from 'hono';
import type { SSEStreamingApi } from 'hono/streaming';
import { z } from 'zod';
import type { AgentConfig } from '@hubness/shared';
import { findAgent } from '../agents/registry.ts';
import { settings } from '../catalog.ts';
import { HttpError } from '../errors.ts';

/**
 * HTTP helpers shared by the routers: request bodies are parsed and checked
 * against a zod schema at the boundary (shape and types); domain functions keep
 * their own checks (does the folder exist, limits with specific messages…).
 */

type BodyOpts = {
  /** No body / not JSON / broken JSON counts as {} (actions that take optional parameters). */
  lenient?: boolean;
};

/** Parse the JSON body with a schema; any mismatch is a 400 that names the field. */
export async function parseBody<S extends z.ZodType>(c: Context, schema: S, opts: BodyOpts = {}): Promise<z.output<S>> {
  let raw: unknown = {};
  const isJson = c.req.header('content-type')?.includes('json');
  if (isJson || !opts.lenient) {
    try {
      raw = await c.req.json();
    } catch {
      if (!opts.lenient) throw new HttpError(400, 'Invalid JSON');
      raw = {};
    }
  }
  const r = schema.safeParse(raw);
  if (!r.success) throw new HttpError(400, describeIssue(r.error));
  return r.data;
}

function describeIssue(e: z.ZodError): string {
  const i = e.issues[0];
  if (!i) return 'Invalid request body';
  const path = i.path.join('.');
  return path ? `${path}: ${i.message}` : i.message;
}

// ---------- common schemas ----------

export const PermissionModeSchema = z.enum(['ask', 'acceptEdits', 'bypass', 'plan']);

export const AgentConfigSchema = z.object({
  kind: z.string().min(1).max(64),
  model: z.string().max(200).optional(),
  permissionMode: PermissionModeSchema.optional(),
  account: z.string().max(120).optional(),
});

/** Terminal size of a job / shell (characters). */
export const TermSizeSchema = z.object({
  cols: z.number().int().min(1).max(1000).optional(),
  rows: z.number().int().min(1).max(500).optional(),
});

/** A short single-line field (names, titles). */
export const shortText = (max = 200) => z.string().max(max);

/** Agent config of a new session: a registered agent; no mode → the hub default (Settings → Agent). */
export function validateAgent(a: z.output<typeof AgentConfigSchema> | AgentConfig | undefined): AgentConfig {
  if (!a || !findAgent(a.kind)) throw new HttpError(400, 'Agent is not specified');
  // new sessions always use the agent's default account (legacy profiles only resume old sessions)
  return { kind: a.kind, model: a.model?.trim() || undefined, permissionMode: a.permissionMode ?? settings().defaultPermission };
}

// ---------- SSE ----------

/**
 * Pump messages into an SSE stream until the client goes away (or `done` says the
 * last message was sent). Every 25 s a keep-alive: `ping()`'s message, or a bare ping event.
 */
export async function pumpSSE<T>(
  stream: SSEStreamingApi,
  subscribe: (push: (m: T) => void) => () => void,
  opts: { first?: T; ping?: () => T; done?: (m: T) => boolean } = {},
): Promise<void> {
  const queue: T[] = [];
  let wake: (() => void) | null = null;
  let closed = false;
  const push = (m: T) => {
    queue.push(m);
    wake?.();
  };
  const off = subscribe(push);
  stream.onAbort(() => {
    closed = true;
    wake?.();
  });
  const timer = setInterval(() => {
    if (opts.ping) push(opts.ping());
    else stream.writeSSE({ event: 'ping', data: '' }).catch(() => {});
  }, 25_000);
  try {
    if (opts.first !== undefined) await stream.writeSSE({ data: JSON.stringify(opts.first) });
    while (!closed) {
      while (queue.length && !closed) {
        const m = queue.shift()!;
        await stream.writeSSE({ data: JSON.stringify(m) });
        if (opts.done?.(m)) closed = true;
      }
      if (!closed) await new Promise<void>((r) => (wake = r));
      wake = null;
    }
  } finally {
    clearInterval(timer);
    off();
  }
}
