import { isIP } from 'node:net';
import type { MiddlewareHandler } from 'hono';
import { config } from './config.ts';
import { tailscaleHosts } from './tailscale.ts';

/**
 * DNS rebinding protection. A page of another site can point its own name at 127.0.0.1 and
 * become same-origin with the hub (the CSRF guard then lets it through). The browser still
 * sends that foreign name as Host, so a request must be addressed to a name the hub is known
 * by: localhost, an IP address (a page can't be same-origin with one it doesn't serve),
 * HUBNESS_HOST, HUBNESS_ALLOWED_HOSTS or the name Tailscale Serve passes to the hub.
 */

/** The name of a Host header: lower case, without the port, the IPv6 brackets or a final dot. */
export function hostName(host: string): string {
  const h = host.trim().toLowerCase();
  const name = h.startsWith('[') ? h.slice(1, h.indexOf(']')) : h.replace(/:\d*$/, '');
  return name.replace(/\.$/, '');
}

/** May a request with this Host header reach the hub (or a preview port)? */
export async function knownHost(host: string | undefined): Promise<boolean> {
  // no Host at all is not a browser (in-process requests, HTTP/1.0 tools)
  if (host === undefined) return true;
  const name = hostName(host);
  if (name === 'localhost' || isIP(name) !== 0 || name === config.host.toLowerCase() || config.allowedHosts.includes(name)) return true;
  return (await tailscaleHosts()).includes(name);
}

export const hostGuard: MiddlewareHandler = async (c, next) => {
  if (await knownHost(c.req.header('host'))) return next();
  return c.json({ error: 'This host name is not allowed: add it to HUBNESS_ALLOWED_HOSTS' }, 403);
};
