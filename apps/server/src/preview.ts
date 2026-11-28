import { createServer, request, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import { connect } from 'node:net';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import type { PreviewLog } from '@hubness/shared';
import { config } from './config.ts';
import { knownHost } from './host.ts';
import { exposePreview, unexposePreview } from './tailscale.ts';

/**
 * Preview of a running app (like Webview in Replit).
 *
 * Each preview is a separate hub port (range HUBNESS_PREVIEW_PORTS,
 * 8790–8809 by default) on the same host as the hub: so it opens from
 * a phone too, even if the app listens only on 127.0.0.1 or lives in
 * a container. Port = separate origin: the app's JS can't read responses of
 * the hub API (and the hub rejects mutating requests without X-Hubness-Request/JSON).
 * The path is not rewritten — the app works from the root (/), assets and HMR
 * (WebSocket) work as is. The Host header is replaced with localhost:port —
 * dev servers (Vite) check it.
 *
 * Once hub login exists, move previews to subdomains (hub cookies are shared
 * across all ports of one host).
 */

type Upstream = { host: string; port: number } | null;
/** Where the app listens now (a container IP may take a docker call to find). */
export type Target = () => Upstream | Promise<Upstream>;

const [FROM, TO] = (process.env.HUBNESS_PREVIEW_PORTS ?? '8790-8809').split('-').map(Number) as [number, number];

interface Preview {
  key: string;
  port: number;
  server: Server;
  target: Target;
  /** When it was last asked for or served a request. */
  used: number;
}

const previews = new Map<string, Preview>();

// ---------- the helper script inside previewed pages, and what it reports ----------

/** The script the proxy adds to every HTML page (console, errors, address bar, element picker). */
const INJECT_JS = readFileSync(fileURLToPath(new URL('../../../packages/shared/assets/preview-inject.js', import.meta.url)), 'utf8');
const INJECT_TAG = '<script src="/__hubness/preview.js"></script>';
/** HUBNESS_PREVIEW_INJECT=0 turns the helper off. */
const injectOn = () => process.env.HUBNESS_PREVIEW_INJECT !== '0';
/** The hub's headless browser (preview_* agent tools) marks itself in the user agent. */
export const AGENT_UA_MARK = 'hubness-agent';
const MAX_LOGS = 500;
/** How much of a page is looked at for the place of the helper before it is passed on as is. */
const HEAD_LIMIT = 64 * 1024;

const logs = new Map<string, PreviewLog[]>();
/** One counter for the whole hub: a cleared log or a restarted preview never goes back to 1 (clients keep a cursor). */
let logSeq = 0;

export function addPreviewLog(key: string, raw: Partial<PreviewLog>, source: PreviewLog['source']) {
  const level = (['error', 'warn', 'log', 'network'] as const).find((l) => l === raw.level) ?? 'log';
  const list = logs.get(key) ?? [];
  logs.set(key, list);
  const text = (v: unknown) => (typeof v === 'string' ? v : v == null ? '' : JSON.stringify(v));
  list.push({
    seq: ++logSeq,
    ts: typeof raw.ts === 'string' ? raw.ts.slice(0, 40) : new Date().toISOString(),
    level,
    text: text(raw.text).slice(0, 4000),
    url: text(raw.url).slice(0, 2000),
    source,
  });
  if (list.length > MAX_LOGS) list.splice(0, list.length - MAX_LOGS);
}

/** Log entries of a preview after `since` (a seq). */
export const previewLogs = (key: string, since = 0): PreviewLog[] => (logs.get(key) ?? []).filter((l) => l.seq > since);
export const clearPreviewLogs = (key: string) => logs.delete(key);
export const previewKeyByPort = (port: number) => [...previews.values()].find((p) => p.port === port)?.key ?? null;

/**
 * Where the helper tag goes in the start of a document: right after `<head>`, else after the
 * doctype or `<html>` it begins with. `null` — this is not a document (a fragment for
 * jQuery / htmx, JSON with a wrong type): it is left alone.
 */
export function injectPoint(html: string): number | null {
  const head = /<head(\s[^>]*)?>/i.exec(html);
  if (head) return head.index + head[0].length;
  const start = /^\s*(<!doctype[^>]*>|<html(\s[^>]*)?>)/i.exec(html);
  return start ? start[0].length : null;
}

/** A whole document with the helper tag (unchanged when it has one, or is not a document). */
export function injectHtml(html: string): string {
  if (html.includes('/__hubness/preview.js')) return html;
  const at = injectPoint(html);
  return at === null ? html : html.slice(0, at) + INJECT_TAG + html.slice(at);
}

/** Is it the browser loading a page (not a script asking for a piece of HTML)? */
function pageRequest(req: IncomingMessage): boolean {
  if (!injectOn() || req.method !== 'GET') return false;
  const dest = req.headers['sec-fetch-dest'];
  if (typeof dest === 'string') return dest === 'document' || dest === 'iframe' || dest === 'frame';
  return String(req.headers.accept ?? '').includes('text/html');
}

/**
 * Only the previewed page itself may report: another site in the user's browser can POST
 * plain text here blindly, and what lands in the log is read by the agent.
 */
function sameOrigin(req: IncomingMessage): boolean {
  const site = req.headers['sec-fetch-site'];
  if (typeof site === 'string' && site !== 'same-origin' && site !== 'none') return false;
  const origin = req.headers.origin;
  if (typeof origin !== 'string') return true;
  try {
    return new URL(origin).host === req.headers.host;
  } catch {
    return false;
  }
}

/** The proxy's own paths: the helper script and its log endpoint. */
function ownRoute(key: string, req: IncomingMessage, res: ServerResponse): boolean {
  const path = (req.url ?? '').split('?')[0];
  if (path === '/__hubness/preview.js' && req.method === 'GET') {
    res.writeHead(200, { 'content-type': 'text/javascript; charset=utf-8', 'cache-control': 'no-store' });
    res.end(INJECT_JS);
    return true;
  }
  if (path === '/__hubness/log' && req.method === 'POST') {
    if (!sameOrigin(req)) {
      req.resume();
      res.writeHead(403);
      res.end();
      return true;
    }
    const source = String(req.headers['user-agent'] ?? '').includes(AGENT_UA_MARK) ? 'agent' : 'user';
    let body = '';
    req.on('data', (d: Buffer) => {
      if (body.length < 256_000) body += d.toString();
    });
    req.on('end', () => {
      try {
        const list = JSON.parse(body);
        for (const e of Array.isArray(list) ? list.slice(0, 50) : []) if (e && typeof e === 'object' && !Array.isArray(e)) addPreviewLog(key, e, source);
      } catch {}
      res.writeHead(204);
      res.end();
    });
    return true;
  }
  return false;
}

/**
 * Pass an HTML answer on with the helper tag in it. Only the start is held back (until
 * `<head>` shows up, or HEAD_LIMIT): a streamed page keeps streaming, a page that never
 * ends is still shown, and bytes are never re-encoded (latin1 maps them 1:1).
 */
function relayHtml(ur: IncomingMessage, res: ServerResponse) {
  let held = '';
  let decided = false;
  const decide = (final: boolean) => {
    if (decided) return;
    const head = /<head(\s[^>]*)?>/i.test(held);
    if (!head && !final && held.length < HEAD_LIMIT) return;
    decided = true;
    const out = injectHtml(held);
    const added = out.length - held.length;
    const headers = { ...ur.headers };
    // a known length grows by the tag; a chunked answer stays chunked
    if (added && headers['content-length'] !== undefined) headers['content-length'] = String(Number(headers['content-length']) + added);
    res.writeHead(ur.statusCode ?? 200, headers);
    res.write(Buffer.from(out, 'latin1'));
    held = '';
  };
  ur.on('data', (d: Buffer) => {
    if (decided) return void res.write(d);
    held += d.toString('latin1');
    decide(false);
  });
  ur.on('end', () => {
    decide(true);
    res.end();
  });
}

function proxyServer(key: string, target: Target, touch: () => void): Server {
  const server = createServer(async (req, res) => {
    touch();
    // a preview port is as open as the hub: the same DNS rebinding protection
    if (!(await knownHost(req.headers.host))) {
      res.writeHead(403, { 'content-type': 'text/plain; charset=utf-8' });
      res.end('This host name is not allowed: add it to HUBNESS_ALLOWED_HOSTS');
      return;
    }
    if (ownRoute(key, req, res)) return;
    const t = await target();
    if (!t) {
      res.writeHead(503, { 'content-type': 'text/plain; charset=utf-8' });
      res.end('The app is not running. Start it in hubness (Run).');
      return;
    }
    // a page (not an asset) comes uncompressed, so the helper can be added to it
    const page = pageRequest(req);
    const headers = {
      ...req.headers,
      host: `localhost:${t.port}`,
      'x-forwarded-host': req.headers.host ?? '',
      'x-forwarded-proto': 'http',
      ...(page ? { 'accept-encoding': 'identity' } : {}),
    };
    const up = request({ host: t.host, port: t.port, method: req.method, path: req.url, headers }, (ur) => {
      // the app died mid-answer: the browser must not wait for the rest forever
      ur.on('error', () => res.destroy());
      ur.on('aborted', () => res.destroy());
      const status = ur.statusCode ?? 502;
      const html = page && status !== 204 && status !== 304 && String(ur.headers['content-type'] ?? '').includes('text/html') && !ur.headers['content-encoding'];
      if (html) return relayHtml(ur, res);
      res.writeHead(status, ur.headers);
      ur.pipe(res);
    });
    up.on('error', (e) => {
      if (res.headersSent) return void res.destroy();
      res.writeHead(502, { 'content-type': 'text/plain; charset=utf-8' });
      res.end(`The app doesn’t respond on port ${t.port} yet (${e.message}). Wait for it to start and reload.`);
    });
    // the browser left (a closed tab, an event stream): let the app's side go too
    res.on('close', () => up.destroy());
    req.pipe(up);
  });
  // WebSocket (HMR etc.): raw passthrough after replacing Host
  server.on('upgrade', async (req, socket, head) => {
    // the client may drop while the target is looked up
    let up: ReturnType<typeof connect> | null = null;
    socket.on('error', () => up?.destroy());
    if (!(await knownHost(req.headers.host))) return socket.destroy();
    const t = await target();
    if (!t || socket.destroyed) return socket.destroy();
    const conn = connect(t.port, t.host, () => {
      const lines = [`${req.method} ${req.url} HTTP/1.1`];
      for (let i = 0; i < req.rawHeaders.length; i += 2) {
        const name = req.rawHeaders[i]!;
        lines.push(`${name}: ${name.toLowerCase() === 'host' ? `localhost:${t.port}` : req.rawHeaders[i + 1]}`);
      }
      conn.write(lines.join('\r\n') + '\r\n\r\n');
      if (head.length) conn.write(head);
      socket.pipe(conn).pipe(socket);
    });
    up = conn;
    conn.on('error', () => socket.destroy());
  });
  return server;
}

function listen(server: Server, port: number): Promise<boolean> {
  return new Promise((resolve) => {
    const onError = () => resolve(false);
    server.once('error', onError);
    server.listen(port, config.host, () => {
      server.off('error', onError);
      resolve(true);
    });
  });
}

/** A preview of a port somebody named (not of a run command): `<project>:port:<n>`. */
const adHoc = (key: string) => key.includes(':port:');

/**
 * Preview port for a key (project:app port); starts the proxy on first request. When every
 * port is taken, the ad-hoc preview nobody has used for the longest time gives its up (an
 * agent trying addresses would otherwise use the range up for good).
 */
export async function ensurePreview(key: string, target: Target): Promise<number | null> {
  const cur = previews.get(key);
  if (cur) {
    cur.target = target;
    cur.used = Date.now();
    // asked for again: one more try if Tailscale Serve refused the port before
    void exposePreview(cur.port);
    return cur.port;
  }
  const open = async (): Promise<number | null> => {
    const taken = new Set([...previews.values()].map((p) => p.port));
    for (let port = FROM; port <= TO; port++) {
      if (taken.has(port)) continue;
      const entry: Preview = { key, port, server: null as unknown as Server, target, used: Date.now() };
      entry.server = proxyServer(key, () => entry.target(), () => (entry.used = Date.now()));
      if (await listen(entry.server, port)) {
        previews.set(key, entry);
        void exposePreview(port);
        return port;
      }
    }
    return null;
  };
  const port = await open();
  if (port !== null) return port;
  const idle = [...previews.values()].filter((p) => adHoc(p.key)).sort((a, b) => a.used - b.used)[0];
  if (!idle) return null;
  closePreview(idle.key);
  return open();
}

export const previewPort = (key: string) => previews.get(key)?.port ?? null;

export function closePreview(key: string) {
  const p = previews.get(key);
  if (!p) return;
  previews.delete(key);
  logs.delete(key);
  p.server.close();
  p.server.closeAllConnections?.();
  void unexposePreview(p.port);
}

export function closePreviewsWithPrefix(prefix: string) {
  for (const key of [...previews.keys()]) if (key.startsWith(prefix)) closePreview(key);
}
