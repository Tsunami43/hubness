import { createServer, request, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import { connect } from 'node:net';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import type { PreviewLog } from '@hubness/shared';
import { config } from './config.ts';

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

const logs = new Map<string, { seq: number; list: PreviewLog[] }>();

export function addPreviewLog(key: string, raw: Partial<PreviewLog>, source: PreviewLog['source']) {
  const level = (['error', 'warn', 'log', 'network'] as const).find((l) => l === raw.level) ?? 'log';
  const box = logs.get(key) ?? { seq: 0, list: [] };
  logs.set(key, box);
  box.list.push({
    seq: ++box.seq,
    ts: typeof raw.ts === 'string' ? raw.ts.slice(0, 40) : new Date().toISOString(),
    level,
    text: String(raw.text ?? '').slice(0, 4000),
    url: String(raw.url ?? '').slice(0, 2000),
    source,
  });
  if (box.list.length > MAX_LOGS) box.list.splice(0, box.list.length - MAX_LOGS);
}

/** Log entries of a preview after `since` (a seq). */
export const previewLogs = (key: string, since = 0): PreviewLog[] => (logs.get(key)?.list ?? []).filter((l) => l.seq > since);
export const clearPreviewLogs = (key: string) => logs.delete(key);
export const previewKeyByPort = (port: number) => [...previews.values()].find((p) => p.port === port)?.key ?? null;

/** Insert the helper tag right after <head> (or at the start of the document). */
export function injectHtml(html: string): string {
  if (html.includes('/__hubness/preview.js')) return html;
  const head = /<head(\s[^>]*)?>/i.exec(html);
  if (head) return html.slice(0, head.index + head[0].length) + INJECT_TAG + html.slice(head.index + head[0].length);
  const doctype = /^\s*<!doctype[^>]*>/i.exec(html);
  return doctype ? doctype[0] + INJECT_TAG + html.slice(doctype[0].length) : INJECT_TAG + html;
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
    const source = String(req.headers['user-agent'] ?? '').includes(AGENT_UA_MARK) ? 'agent' : 'user';
    let body = '';
    req.on('data', (d: Buffer) => {
      if (body.length < 256_000) body += d.toString();
    });
    req.on('end', () => {
      try {
        const list = JSON.parse(body);
        for (const e of Array.isArray(list) ? list.slice(0, 50) : []) if (e && typeof e === 'object') addPreviewLog(key, e, source);
      } catch {}
      res.writeHead(204);
      res.end();
    });
    return true;
  }
  return false;
}

function proxyServer(key: string, target: Target): Server {
  const server = createServer(async (req, res) => {
    if (ownRoute(key, req, res)) return;
    const t = await target();
    if (!t) {
      res.writeHead(503, { 'content-type': 'text/plain; charset=utf-8' });
      res.end('The app is not running. Start it in hubness (Run).');
      return;
    }
    // a page (not an asset) comes uncompressed, so the helper can be added to it
    const page = injectOn() && req.method === 'GET' && String(req.headers.accept ?? '').includes('text/html');
    const headers = {
      ...req.headers,
      host: `localhost:${t.port}`,
      'x-forwarded-host': req.headers.host ?? '',
      'x-forwarded-proto': 'http',
      ...(page ? { 'accept-encoding': 'identity' } : {}),
    };
    const up = request({ host: t.host, port: t.port, method: req.method, path: req.url, headers }, (ur) => {
      const html = page && String(ur.headers['content-type'] ?? '').includes('text/html') && !ur.headers['content-encoding'];
      if (!html) {
        res.writeHead(ur.statusCode ?? 502, ur.headers);
        ur.pipe(res);
        return;
      }
      const chunks: Buffer[] = [];
      ur.on('data', (d: Buffer) => chunks.push(d));
      ur.on('end', () => {
        // latin1 maps bytes 1:1, so a page in any encoding keeps its bytes
        const body = Buffer.from(injectHtml(Buffer.concat(chunks).toString('latin1')), 'latin1');
        const { 'content-length': _l, ...rest } = ur.headers;
        res.writeHead(ur.statusCode ?? 200, { ...rest, 'content-length': String(body.length) });
        res.end(body);
      });
      ur.on('error', () => res.destroy());
    });
    up.on('error', (e) => {
      if (!res.headersSent) res.writeHead(502, { 'content-type': 'text/plain; charset=utf-8' });
      res.end(`The app doesn’t respond on port ${t.port} yet (${e.message}). Wait for it to start and reload.`);
    });
    req.pipe(up);
  });
  // WebSocket (HMR etc.): raw passthrough after replacing Host
  server.on('upgrade', async (req, socket, head) => {
    // the client may drop while the target is looked up
    let up: ReturnType<typeof connect> | null = null;
    socket.on('error', () => up?.destroy());
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

/** Preview port for a key (project:app port); starts the proxy on first request. */
export async function ensurePreview(key: string, target: Target): Promise<number | null> {
  const cur = previews.get(key);
  if (cur) {
    cur.target = target;
    return cur.port;
  }
  const used = new Set([...previews.values()].map((p) => p.port));
  for (let port = FROM; port <= TO; port++) {
    if (used.has(port)) continue;
    const entry: Preview = { key, port, server: null as unknown as Server, target };
    entry.server = proxyServer(key, () => entry.target());
    if (await listen(entry.server, port)) {
      previews.set(key, entry);
      return port;
    }
  }
  return null;
}

export const previewPort = (key: string) => previews.get(key)?.port ?? null;

export function closePreview(key: string) {
  const p = previews.get(key);
  if (!p) return;
  previews.delete(key);
  logs.delete(key);
  p.server.close();
  p.server.closeAllConnections?.();
}

export function closePreviewsWithPrefix(prefix: string) {
  for (const key of [...previews.keys()]) if (key.startsWith(prefix)) closePreview(key);
}
