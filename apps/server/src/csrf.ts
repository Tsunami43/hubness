import type { MiddlewareHandler } from 'hono';

/**
 * CSRF protection: a mutating API request must be JSON or carry the
 * X-Hubness-Request header. A browser won't send such a request from a foreign
 * origin (another site, an app in preview) without a CORS preflight, and the hub
 * doesn't allow preflight. Without this, any open page could use a "simple" POST
 * (text/plain, a form) to run a command via Run/Shell or the agent.
 */
export const csrfGuard: MiddlewareHandler = async (c, next) => {
  if (['GET', 'HEAD', 'OPTIONS'].includes(c.req.method)) return next();
  const type = c.req.header('content-type') ?? '';
  if (!/^application\/json\b/i.test(type) && !c.req.header('x-hubness-request'))
    return c.json({ error: 'Requests that change data must be JSON or carry X-Hubness-Request' }, 403);
  return next();
};
