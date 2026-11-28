import { existsSync, readFileSync } from 'node:fs';
import { relative } from 'node:path';
import { serve } from '@hono/node-server';
import { serveStatic } from '@hono/node-server/serve-static';
import { Hono } from 'hono';
import { config, isLoopback } from './config.ts';
import { HttpError } from './errors.ts';
import { csrfGuard } from './csrf.ts';
import { api } from './routes/api.ts';
import { harness, jobs } from './routes/harness.ts';
import { handleAsk, system } from './routes/system.ts';
import { ensureHomeProject } from './system/ask.ts';
import { stopAll } from './runtime.ts';
import { shutdownBrowser } from './browser.ts';
import { disposeAllSsh, sweepSshRuntime } from './ssh.ts';
import { killAllJobs } from './terminal.ts';
import { initTailscale, unexposeAllPreviews } from './tailscale.ts';

// this server has no sign-in: it never listens where other machines can reach it
if (!isLoopback(config.host)) {
  console.error(
    `hubness: HUBNESS_HOST=${config.host} is reachable from other machines and this server has no sign-in; ` +
      'keep 127.0.0.1 (use an SSH tunnel or Tailscale Serve for remote access) or run the Rust server with an account',
  );
  process.exit(1);
}

const app = new Hono();

app.onError((err, c) => {
  if (err instanceof HttpError) return c.json({ error: err.message }, err.status as 400);
  console.error(err);
  return c.json({ error: err.message || 'Internal error' }, 500);
});

app.use('/api/*', csrfGuard);

app.route('/api/system', system);
app.route('/api/harness', harness);
app.route('/api/jobs', jobs);
app.post('/api/ask', handleAsk);
app.route('/api', api);
app.all('/api/*', (c) => c.json({ error: 'Not found' }, 404));

// The built frontend (pnpm build) is served from the same origin.
if (existsSync(config.webDist)) {
  const root = relative(process.cwd(), config.webDist) || '.';
  app.use('/*', serveStatic({ root }));
  const indexHtml = () => readFileSync(`${config.webDist}/index.html`, 'utf8');
  app.get('*', (c) => c.html(indexHtml()));
}

ensureHomeProject();

// agents and key scratch folders left by a crashed run
sweepSshRuntime();

// previews of a hub behind Tailscale Serve get their ports passed through it
initTailscale(config.port, config.host);

const server = serve({ fetch: app.fetch, port: config.port, hostname: config.host }, (info) => {
  console.log(`hubness: http://${info.address}:${info.port}  (data: ${config.dataDir})`);
});

const shutdown = () => {
  stopAll();
  // the turns' ssh-agents hold decrypted keys: they never outlive the hub
  disposeAllSsh();
  shutdownBrowser();
  killAllJobs();
  unexposeAllPreviews();
  server.close();
  // SSE streams never end on their own: drop them so the dev restart (src/dev.ts) is instant —
  // edits made while the old process is still exiting are otherwise missed by the watcher
  (server as { closeAllConnections?: () => void }).closeAllConnections?.();
  setTimeout(() => process.exit(0), 300).unref();
};
process.on('SIGINT', shutdown);
process.on('SIGTERM', shutdown);
