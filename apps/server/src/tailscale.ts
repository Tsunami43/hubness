import { execFile, execFileSync } from 'node:child_process';

/**
 * Previews behind Tailscale Serve. A hub opened as `https://<machine>.<tailnet>.ts.net`
 * (`tailscale serve` → the hub's port) is reached only through the ports Serve passes, and a
 * preview is a port of its own: the hub adds a Serve route for a preview port when the preview
 * starts and removes it when the preview stops. (Subdomains per preview are not an option
 * there: MagicDNS and the certificate cover the machine's own name only.)
 *
 * Needs the right to change Serve: `sudo tailscale set --operator=$USER`, once. Without it the
 * route is not added and the Preview tab says what to run. `HUBNESS_TAILSCALE` names the
 * `tailscale` binary (tests), `0` turns this off.
 */

export interface ServeFront {
  /** Serve terminates TLS for the hub (`--https`), else plain HTTP (`--http`). */
  https: boolean;
}

const LOOPBACK = ['127.0.0.1', 'localhost', '[::1]'];

type ServeStatus = { TCP?: Record<string, { HTTPS?: boolean }>; Web?: Record<string, { Handlers?: Record<string, { Proxy?: string }> }> } | null;

/** The `<name>:<port>` sites of `tailscale serve status --json` that pass to the hub's port. */
function hubSites(status: unknown, hubPort: number): string[] {
  return Object.entries((status as ServeStatus)?.Web ?? {})
    .filter(([, web]) =>
      Object.values(web?.Handlers ?? {}).some((h) => {
        const m = /^https?:\/\/([^/]+?):(\d+)\/?$/.exec(h?.Proxy ?? '');
        return m !== null && LOOPBACK.includes(m[1]!) && Number(m[2]) === hubPort;
      }),
    )
    .map(([hostPort]) => hostPort);
}

/** Is the hub's port served by Tailscale Serve? Reads `tailscale serve status --json`. */
export function serveFront(status: unknown, hubPort: number): ServeFront | null {
  const hostPort = hubSites(status, hubPort)[0];
  if (hostPort === undefined) return null;
  return { https: (status as ServeStatus)?.TCP?.[hostPort.slice(hostPort.lastIndexOf(':') + 1)]?.HTTPS === true };
}

/** The names Tailscale Serve reaches the hub by (`<machine>.<tailnet>.ts.net`). */
export function serveHosts(status: unknown, hubPort: number): string[] {
  return hubSites(status, hubPort).map((hostPort) => hostPort.slice(0, hostPort.lastIndexOf(':')).toLowerCase());
}

let hub: { port: number; host: string } | null = null;
let front: { at: number; value: ServeFront | null; hosts: string[] } | null = null;
const exposed = new Map<number, ServeFront>();
let warned = false;
// one tailscale command at a time, in the order asked for
let queue: Promise<void> = Promise.resolve();

const FRONT_TTL_MS = 30_000;
const tool = () => process.env.HUBNESS_TAILSCALE || 'tailscale';
const on = () => hub !== null && tool() !== '0';

/** Called once by the server (not by tests of other modules: then nothing here runs). */
export function initTailscale(port: number, host: string) {
  hub = { port, host };
  front = null;
}

const run = (args: string[]) =>
  new Promise<{ ok: boolean; out: string }>((resolve) =>
    execFile(tool(), args, { timeout: 10_000 }, (err, stdout, stderr) => resolve({ ok: !err, out: `${stdout}${stderr}`.trim() })),
  );

async function currentFront(): Promise<ServeFront | null> {
  if (front && Date.now() - front.at < FRONT_TTL_MS) return front.value;
  const r = await run(['serve', 'status', '--json']);
  let value: ServeFront | null = null;
  let hosts: string[] = [];
  try {
    if (r.ok) {
      const status = JSON.parse(r.out || '{}');
      value = serveFront(status, hub!.port);
      hosts = serveHosts(status, hub!.port);
    }
  } catch {}
  front = { at: Date.now(), value, hosts };
  return value;
}

/** The names Tailscale Serve passes to the hub (checked by the Host guard for a name it doesn't know). */
export async function tailscaleHosts(): Promise<string[]> {
  if (!on()) return [];
  await enqueue(async () => void (await currentFront()));
  return front?.hosts ?? [];
}

const flag = (f: ServeFront, port: number) => `--${f.https ? 'https' : 'http'}=${port}`;
/** Where Serve finds the preview proxy: the hub's own address (loopback when it listens everywhere). */
const upstream = (port: number) => `http://${['0.0.0.0', '::'].includes(hub!.host) ? '127.0.0.1' : hub!.host}:${port}`;

function enqueue(job: () => Promise<void>): Promise<void> {
  queue = queue.then(job, job).catch(() => {});
  return queue;
}

/** Let the preview port through Serve (nothing to do when the hub is not behind Serve). */
export function exposePreview(port: number): Promise<void> {
  if (!on()) return Promise.resolve();
  return enqueue(async () => {
    if (exposed.has(port)) return;
    const f = await currentFront();
    if (!f) return;
    const r = await run(['serve', '--bg', flag(f, port), upstream(port)]);
    if (r.ok) exposed.set(port, f);
    else if (!warned) {
      warned = true;
      console.warn(`hubness: could not open preview port ${port} in Tailscale Serve (${r.out.split('\n')[0]}); run \`sudo tailscale set --operator=$USER\` once`);
    }
  });
}

/** Take the preview port's Serve route away again. */
export function unexposePreview(port: number): Promise<void> {
  if (!on()) return Promise.resolve();
  return enqueue(async () => {
    const f = exposed.get(port);
    if (!f) return;
    exposed.delete(port);
    await run(['serve', flag(f, port), 'off']);
  });
}

/** On hub exit: no route of ours stays behind. */
export function unexposeAllPreviews() {
  if (!on()) return;
  for (const [port, f] of exposed) {
    try {
      execFileSync(tool(), ['serve', flag(f, port), 'off'], { timeout: 5000, stdio: 'ignore' });
    } catch {}
  }
  exposed.clear();
}
