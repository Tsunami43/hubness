/**
 * What the Preview tab shows while there is no page to show, and whether the browser can
 * reach a preview port at all (pure, tested from the server tests).
 */

/** The part of a run command's state the Preview tab reads. */
export interface RunLike {
  id: string;
  name: string;
  primary: boolean;
  port: number | null;
  activePort: number | null;
  previewPort: number | null;
  job: { status: 'running' | 'exited'; exitCode: number | null } | null;
}

export type PreviewIdle<R extends RunLike> =
  /** A command runs, but its port is not known yet (nothing printed, no port set). */
  | { kind: 'waiting'; run: R }
  /** The last run of a command ended with an error. */
  | { kind: 'failed'; run: R; code: number }
  | { kind: 'idle' };

/** Why there is nothing to preview: the running command comes first, then a failed one. */
export function previewIdle<R extends RunLike>(runs: R[]): PreviewIdle<R> {
  const waiting = runs.find((r) => r.job?.status === 'running' && !r.previewPort);
  if (waiting) return { kind: 'waiting', run: waiting };
  const order = [...runs].sort((a, b) => Number(b.primary) - Number(a.primary));
  const failed = order.find((r) => r.job?.status === 'exited' && Boolean(r.job.exitCode));
  if (failed) return { kind: 'failed', run: failed, code: failed.job!.exitCode! };
  return { kind: 'idle' };
}

/**
 * Commands the Preview tab offers to start: the ones known to listen on a port; when no
 * command has a port yet, the main one (its port shows up in its output).
 */
export function startable<R extends RunLike>(runs: R[]): R[] {
  const stopped = runs.filter((r) => r.job?.status !== 'running');
  const withPort = stopped.filter((r) => r.port || r.activePort);
  if (withPort.length) return withPort;
  const main = stopped.find((r) => r.primary) ?? stopped[0];
  return main && !runs.some((r) => r.port || r.activePort) ? [main] : [];
}

/**
 * Can this browser open the preview port? Every preview is its own port of the hub: an HTTPS
 * proxy or a VPN front (Tailscale Serve, Caddy, nginx) that passes only the hub's port leaves
 * it closed, and the frame would stay blank. Any answer counts (a sign-in page too); only a
 * failed or hanging connection means "no". Asked a few times: behind Tailscale Serve the hub
 * opens the port itself, a moment after the preview starts.
 */
export async function reachable(origin: string, fetcher: typeof fetch = fetch, { timeoutMs = 5000, attempts = 4, pauseMs = 1000 } = {}): Promise<boolean> {
  for (let n = 0; n < attempts; n++) {
    if (n) await new Promise((r) => setTimeout(r, pauseMs));
    const stop = new AbortController();
    const timer = setTimeout(() => stop.abort(), timeoutMs);
    try {
      await fetcher(`${origin}/__hubness/preview.js`, { mode: 'no-cors', cache: 'no-store', credentials: 'omit', signal: stop.signal });
      return true;
    } catch {
      // closed for now
    } finally {
      clearTimeout(timer);
    }
  }
  return false;
}

/**
 * On a Tailscale Serve address (`*.ts.net`) the hub opens preview ports itself, once its user
 * may change Serve: the command that allows it (run once on the hub's machine), else null.
 */
export function tailscaleOperatorCommand(hostname: string): string | null {
  return hostname.toLowerCase().endsWith('.ts.net') ? 'sudo tailscale set --operator=$USER' : null;
}

/**
 * An address on the hub's own machine (`http://localhost:5173/x` — how an agent names the app
 * it runs): its port and the rest. In the user's browser `localhost` is the user's machine,
 * so such a page is shown through the project's preview. Null for any other address.
 */
export function localApp(url: string): { port: number; path: string } | null {
  let u: URL;
  try {
    u = new URL(url);
  } catch {
    return null;
  }
  if (u.protocol !== 'http:' && u.protocol !== 'https:') return null;
  if (!['localhost', '127.0.0.1', '0.0.0.0', '[::1]'].includes(u.hostname)) return null;
  const port = u.port ? Number(u.port) : u.protocol === 'https:' ? 443 : 80;
  return { port, path: `${u.pathname}${u.search}${u.hash}` };
}
