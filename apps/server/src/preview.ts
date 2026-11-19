import { createServer, request, type Server } from 'node:http';
import { connect } from 'node:net';
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

export type Target = () => { host: string; port: number } | null;

const [FROM, TO] = (process.env.HUBNESS_PREVIEW_PORTS ?? '8790-8809').split('-').map(Number) as [number, number];

interface Preview {
  key: string;
  port: number;
  server: Server;
  target: Target;
}

const previews = new Map<string, Preview>();

function proxyServer(target: Target): Server {
  const server = createServer((req, res) => {
    const t = target();
    if (!t) {
      res.writeHead(503, { 'content-type': 'text/plain; charset=utf-8' });
      res.end('The app is not running. Start it in hubness (Run).');
      return;
    }
    const headers = { ...req.headers, host: `localhost:${t.port}`, 'x-forwarded-host': req.headers.host ?? '', 'x-forwarded-proto': 'http' };
    const up = request({ host: t.host, port: t.port, method: req.method, path: req.url, headers }, (ur) => {
      res.writeHead(ur.statusCode ?? 502, ur.headers);
      ur.pipe(res);
    });
    up.on('error', (e) => {
      if (!res.headersSent) res.writeHead(502, { 'content-type': 'text/plain; charset=utf-8' });
      res.end(`The app doesn’t respond on port ${t.port} yet (${e.message}). Wait for it to start and reload.`);
    });
    req.pipe(up);
  });
  // WebSocket (HMR etc.): raw passthrough after replacing Host
  server.on('upgrade', (req, socket, head) => {
    const t = target();
    if (!t) return socket.destroy();
    const up = connect(t.port, t.host, () => {
      const lines = [`${req.method} ${req.url} HTTP/1.1`];
      for (let i = 0; i < req.rawHeaders.length; i += 2) {
        const name = req.rawHeaders[i]!;
        lines.push(`${name}: ${name.toLowerCase() === 'host' ? `localhost:${t.port}` : req.rawHeaders[i + 1]}`);
      }
      up.write(lines.join('\r\n') + '\r\n\r\n');
      if (head.length) up.write(head);
      socket.pipe(up).pipe(socket);
    });
    up.on('error', () => socket.destroy());
    socket.on('error', () => up.destroy());
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
    entry.server = proxyServer(() => entry.target());
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
  p.server.close();
  p.server.closeAllConnections?.();
}

export function closePreviewsWithPrefix(prefix: string) {
  for (const key of [...previews.keys()]) if (key.startsWith(prefix)) closePreview(key);
}
