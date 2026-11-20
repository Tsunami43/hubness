import { readFileSync } from 'node:fs';
import { cpus, freemem, hostname, loadavg, totalmem, uptime } from 'node:os';
import type { ContainerInfo, DockerNetwork, DockerVolume, SystemOverview, UnitInfo } from '@hubness/shared';
import { HttpError } from '../errors.ts';
import { sh } from './exec.ts';
import { hostInfo, PLATFORM, sensors, systemPlatform } from './host.ts';

export { sh, type CmdResult } from './exec.ts';

/**
 * Server monitoring: system metrics, Docker, services (systemd or launchd), packages.
 * Only official CLIs, node:os and /proc; nothing requires root (system units are
 * read-only with logs; user units/agents can be managed). Linux and macOS.
 */

const tail = (s: string, n: number) => s.trim().split('\n').slice(-n).join('\n');

// ---------- overview ----------

let lastCpu: { idle: number; total: number } | null = null;

/** Aggregate CPU times from node:os (works on Linux and macOS alike). */
function cpuSample(): { idle: number; total: number } | null {
  const list = cpus();
  if (!list.length) return null;
  let idle = 0;
  let total = 0;
  for (const c of list) {
    idle += c.times.idle;
    total += c.times.user + c.times.nice + c.times.sys + c.times.irq + c.times.idle;
  }
  return { idle, total };
}

function cpuPercent(): number | null {
  const cur = cpuSample();
  if (!cur) return null;
  const prev = lastCpu;
  lastCpu = cur;
  if (!prev || cur.total === prev.total) return null;
  return Math.round(100 * (1 - (cur.idle - prev.idle) / (cur.total - prev.total)));
}

interface MemFacts {
  totalMb: number;
  availableMb: number;
  swapTotalMb: number;
  swapUsedMb: number;
}

function linuxMemory(): MemFacts {
  const m: Record<string, number> = {};
  try {
    for (const l of readFileSync('/proc/meminfo', 'utf8').split('\n')) {
      const r = l.match(/^(\w+):\s+(\d+)/);
      if (r) m[r[1]] = Number(r[2]);
    }
  } catch {}
  const mb = (kb: number | undefined) => Math.round((kb ?? 0) / 1024);
  return {
    totalMb: mb(m.MemTotal) || Math.round(totalmem() / 1048576),
    availableMb: mb(m.MemAvailable) || Math.round(freemem() / 1048576),
    swapTotalMb: mb(m.SwapTotal),
    swapUsedMb: mb((m.SwapTotal ?? 0) - (m.SwapFree ?? 0)),
  };
}

/** `vm_stat` pages: free + inactive + speculative + purgeable ≈ what Activity Monitor calls available. */
export function parseVmStat(out: string): number {
  const page = Number(out.match(/page size of (\d+) bytes/)?.[1] ?? 4096);
  const pages = (name: string) => Number(out.match(new RegExp(`Pages ${name}:\\s+(\\d+)`))?.[1] ?? 0);
  return ((pages('free') + pages('inactive') + pages('speculative') + pages('purgeable')) * page) / 1048576;
}

/** `vm.swapusage: total = 2048.00M  used = 1024.50M  free = …` (sysctl -n drops the key). */
export function parseSwapUsage(out: string): { totalMb: number; usedMb: number } {
  const val = (k: string) => {
    const m = out.match(new RegExp(`${k} = ([\\d.]+)([KMG])`));
    if (!m) return 0;
    return Number(m[1]) * ({ K: 1 / 1024, M: 1, G: 1024 } as Record<string, number>)[m[2]];
  };
  return { totalMb: Math.round(val('total')), usedMb: Math.round(val('used')) };
}

async function darwinMemory(): Promise<MemFacts> {
  const [vm, swap] = await Promise.all([sh('vm_stat', []), sh('sysctl', ['-n', 'vm.swapusage'])]);
  const s = parseSwapUsage(swap.out);
  return {
    totalMb: Math.round(totalmem() / 1048576),
    availableMb: Math.round(vm.code === 0 ? parseVmStat(vm.out) : freemem() / 1048576),
    swapTotalMb: s.totalMb,
    swapUsedMb: s.usedMb,
  };
}

async function memory(): Promise<MemFacts> {
  if (PLATFORM === 'linux') return linuxMemory();
  if (PLATFORM === 'darwin') return darwinMemory();
  return { totalMb: Math.round(totalmem() / 1048576), availableMb: Math.round(freemem() / 1048576), swapTotalMb: 0, swapUsedMb: 0 };
}

/** macOS: the sealed system snapshot and its helper volumes are noise; `/` and real volumes stay. */
const DARWIN_HIDDEN = /^\/System\/Volumes\/(?!Data$)|^\/private\/var\/vm/;

export async function disks(): Promise<SystemOverview['disks']> {
  const r =
    PLATFORM === 'darwin'
      ? await sh('df', ['-P', '-k', '-l'])
      : await sh('df', ['-P', '-k', '-x', 'tmpfs', '-x', 'devtmpfs', '-x', 'overlay', '-x', 'squashfs', '-x', 'efivarfs']);
  return r.out
    .split('\n')
    .slice(1)
    .filter(Boolean)
    .map((l) => l.trim().split(/\s+/))
    .filter((c) => c.length >= 6 && c[0].startsWith('/'))
    .map((c) => ({
      device: c[0],
      mount: c.slice(5).join(' '),
      sizeGb: Math.round(Number(c[1]) / 1024 / 1024),
      usedGb: Math.round(Number(c[2]) / 1024 / 1024),
      usedPercent: Number(c[4].replace('%', '')),
    }))
    .filter((d) => !(PLATFORM === 'darwin' && DARWIN_HIDDEN.test(d.mount)));
}

/** Host metrics and static facts: Docker and services are asked only on their own pages. */
export async function overview(): Promise<SystemOverview> {
  const [info, mem, d, sens] = await Promise.all([hostInfo(), memory(), disks(), sensors()]);
  const l = loadavg();
  // the first CPU sample is the baseline; with frequent requests (UI every 10 s) the percentage is real
  const cpu = cpuPercent() ?? (await new Promise<number | null>((r) => setTimeout(() => r(cpuPercent()), 300)));
  return {
    info,
    hostname: hostname(),
    uptimeSec: Math.round(uptime()),
    load: [l[0], l[1], l[2]],
    cpus: cpus().length,
    cpuPercent: cpu,
    gpus: sens.gpus,
    temps: sens.temps,
    memory: { totalMb: mem.totalMb, availableMb: mem.availableMb, usedPercent: mem.totalMb ? Math.round(100 * (1 - mem.availableMb / mem.totalMb)) : 0 },
    swap: { totalMb: mem.swapTotalMb, usedMb: mem.swapUsedMb },
    disks: d,
  };
}

// ---------- Docker ----------

export async function dockerContainers(): Promise<ContainerInfo[] | null> {
  const ps = await sh('docker', ['ps', '-a', '--format', '{{json .}}']);
  if (ps.code !== 0) return null;
  const rows = ps.out
    .split('\n')
    .filter(Boolean)
    .map((l) => JSON.parse(l) as Record<string, string>);
  if (!rows.length) return [];
  const ins = await sh('docker', ['inspect', ...rows.map((r) => r.ID)]);
  const details = new Map<string, any>();
  if (ins.code === 0) for (const d of JSON.parse(ins.out) as any[]) details.set(String(d.Id).slice(0, 12), d);
  return rows.map((r) => {
    const d = details.get(r.ID.slice(0, 12));
    const mem = d?.HostConfig?.Memory ?? 0;
    return {
      id: r.ID,
      name: r.Names,
      image: r.Image,
      state: d?.State?.Status ?? r.State,
      status: r.Status,
      health: d?.State?.Health?.Status ?? null,
      restartCount: d?.RestartCount ?? 0,
      oomKilled: Boolean(d?.State?.OOMKilled),
      exitCode: d?.State?.ExitCode ?? 0,
      ports: r.Ports ?? '',
      memLimitMb: mem ? Math.round(mem / 1024 / 1024) : null,
      finishedAt: d?.State?.FinishedAt && !String(d.State.FinishedAt).startsWith('0001') ? d.State.FinishedAt : null,
    };
  });
}

export async function containerAction(name: string, action: 'start' | 'stop' | 'restart') {
  const r = await sh('docker', [action, name], 60_000);
  return { ok: r.code === 0, output: (r.out + r.err).trim() };
}

const DOCKER_NAME = /^[\w][\w.-]*$/;
const dockerName = (n: string) => {
  if (!DOCKER_NAME.test(n)) throw new HttpError(400, 'Invalid name');
  return n;
};

/** Remove a container — only a stopped one (never `-f`, volumes stay). */
export async function containerRemove(name: string) {
  const ins = await sh('docker', ['inspect', '--format', '{{.State.Status}}', dockerName(name)]);
  if (ins.code !== 0) throw new HttpError(404, 'Container not found');
  const state = ins.out.trim();
  if (state === 'running' || state === 'restarting' || state === 'paused') throw new HttpError(409, 'Stop the container first — only stopped containers can be removed');
  const r = await sh('docker', ['rm', name], 60_000);
  return { ok: r.code === 0, output: (r.out + r.err).trim() };
}

export async function dockerNetworks(): Promise<DockerNetwork[] | null> {
  const ls = await sh('docker', ['network', 'ls', '-q']);
  if (ls.code !== 0) return null;
  const ids = ls.out.split('\n').filter(Boolean);
  if (!ids.length) return [];
  const ins = await sh('docker', ['network', 'inspect', ...ids]);
  if (ins.code !== 0) return null;
  return (JSON.parse(ins.out) as any[])
    .map((n) => ({
      id: String(n.Id).slice(0, 12),
      name: n.Name,
      driver: n.Driver,
      scope: n.Scope,
      subnet: n.IPAM?.Config?.[0]?.Subnet ?? null,
      containers: Object.values(n.Containers ?? {}).map((c: any) => c.Name as string),
      project: n.Labels?.['com.docker.compose.project'] ?? null,
      builtin: ['bridge', 'host', 'none'].includes(n.Name),
      internal: Boolean(n.Internal),
      createdAt: n.Created,
    }))
    .sort((a, b) => Number(b.builtin) - Number(a.builtin) || a.name.localeCompare(b.name));
}

/** Remove a network — only a user-created one with nothing attached. */
export async function networkRemove(name: string) {
  if (['bridge', 'host', 'none'].includes(dockerName(name))) throw new HttpError(409, 'Docker’s own networks can’t be removed');
  const ins = await sh('docker', ['network', 'inspect', name]);
  if (ins.code !== 0) throw new HttpError(404, 'Network not found');
  const attached = Object.keys((JSON.parse(ins.out) as any[])[0]?.Containers ?? {}).length;
  if (attached) throw new HttpError(409, 'Containers are attached to this network');
  const r = await sh('docker', ['network', 'rm', name]);
  return { ok: r.code === 0, output: (r.out + r.err).trim() };
}

/** Volumes with usage and size (docker system df -v — sizes cost a disk walk, so only on demand). */
export async function dockerVolumes(): Promise<DockerVolume[] | null> {
  const ls = await sh('docker', ['volume', 'ls', '--format', '{{json .}}']);
  if (ls.code !== 0) return null;
  const df = await sh('docker', ['system', 'df', '-v', '--format', '{{json .}}'], 60_000);
  const usage = new Map<string, { links: number; size: string }>();
  try {
    for (const v of (JSON.parse(df.out) as { Volumes?: { Name: string; Links: string; Size: string }[] }).Volumes ?? []) usage.set(v.Name, { links: Number(v.Links) || 0, size: v.Size });
  } catch {}
  return ls.out
    .split('\n')
    .filter(Boolean)
    .map((l) => JSON.parse(l) as Record<string, string>)
    .map((v) => {
      const labels = Object.fromEntries((v.Labels ?? '').split(',').filter(Boolean).map((kv) => kv.split('=') as [string, string]));
      const u = usage.get(v.Name);
      return {
        name: v.Name,
        driver: v.Driver,
        links: u?.links ?? 0,
        size: u?.size ?? null,
        project: labels['com.docker.compose.project'] ?? null,
        anonymous: 'com.docker.volume.anonymous' in labels || /^[0-9a-f]{64}$/.test(v.Name),
        mountpoint: v.Mountpoint,
      };
    })
    .sort((a, b) => Number(a.anonymous) - Number(b.anonymous) || a.name.localeCompare(b.name));
}

/** Remove a volume — only one no container (running or stopped) uses. */
export async function volumeRemove(name: string) {
  const users = await sh('docker', ['ps', '-a', '-q', '--filter', `volume=${dockerName(name)}`]);
  if (users.code !== 0) throw new HttpError(500, users.err.trim() || 'docker failed');
  if (users.out.trim()) throw new HttpError(409, 'A container uses this volume — remove the container first');
  const r = await sh('docker', ['volume', 'rm', name]);
  return { ok: r.code === 0, output: (r.out + r.err).trim() };
}

export async function containerLogs(name: string, lines = 200): Promise<string> {
  const r = await sh('docker', ['logs', '--tail', String(lines), '--timestamps', name]);
  return (r.out + r.err).trim();
}

export async function containerInspectSummary(name: string): Promise<string> {
  const r = await sh('docker', ['inspect', name]);
  if (r.code !== 0) return r.err.trim();
  const d = (JSON.parse(r.out) as any[])[0] ?? {};
  const health = d.State?.Health;
  return JSON.stringify(
    {
      Name: d.Name,
      Image: d.Config?.Image,
      State: { ...d.State, Health: health ? { Status: health.Status, FailingStreak: health.FailingStreak, Last: health.Log?.slice(-3) } : undefined },
      RestartCount: d.RestartCount,
      RestartPolicy: d.HostConfig?.RestartPolicy,
      MemoryLimit: d.HostConfig?.Memory,
      CpuLimit: d.HostConfig?.NanoCpus,
      Mounts: d.Mounts?.map((m: any) => `${m.Source} -> ${m.Destination}`),
      Ports: d.NetworkSettings?.Ports,
      Labels: Object.fromEntries(Object.entries(d.Config?.Labels ?? {}).filter(([k]) => k.startsWith('com.docker.compose'))),
    },
    null,
    2,
  );
}

// ---------- systemd ----------

async function listUnits(scope: 'system' | 'user', state?: 'failed'): Promise<UnitInfo[]> {
  const args = [...(scope === 'user' ? ['--user'] : []), 'list-units', '--type=service', '--all', '--no-pager', '--output=json'];
  if (state) args.push(`--state=${state}`);
  const r = await sh('systemctl', args);
  if (r.code !== 0 || !r.out.trim()) return [];
  try {
    return (JSON.parse(r.out) as any[])
      .filter((u) => u.load !== 'not-found')
      .map((u) => ({ name: u.unit, scope, load: u.load, active: u.active, sub: u.sub, description: u.description }));
  } catch {
    return [];
  }
}

export async function units(state?: 'failed'): Promise<UnitInfo[]> {
  const services = systemPlatform().services;
  if (services === 'launchd') return launchdAgents();
  if (services !== 'systemd') throw new HttpError(501, 'No supported service manager on this server (systemd or launchd)');
  const [sys, user] = await Promise.all([listUnits('system', state), listUnits('user', state)]);
  return [...sys, ...user];
}

const unitName = (name: string) => (/^[\w@.:-]+$/.test(name) ? name : null);

export async function unitAction(name: string, scope: 'system' | 'user', action: 'start' | 'stop' | 'restart') {
  if (!unitName(name)) return { ok: false, output: 'Invalid unit name' };
  if (scope === 'system')
    return { ok: false, output: 'System units need root. Ask the agent, or run with sudo in a terminal.' };
  if (systemPlatform().services === 'launchd') return launchdAction(name, action);
  const r = await sh('systemctl', ['--user', action, name], 60_000);
  return { ok: r.code === 0, output: (r.out + r.err).trim() || 'OK' };
}

export async function unitLogs(name: string, scope: 'system' | 'user', lines = 200): Promise<string> {
  if (!unitName(name)) return 'Invalid unit name';
  if (systemPlatform().services === 'launchd') return launchdLogs(name, lines);
  const r = await sh('journalctl', [...(scope === 'user' ? ['--user'] : []), '-u', name, '-n', String(lines), '--no-pager', '-o', 'short-iso']);
  return (r.out + r.err).trim();
}

export async function unitStatus(name: string, scope: 'system' | 'user'): Promise<string> {
  if (!unitName(name)) return 'Invalid unit name';
  if (systemPlatform().services === 'launchd') return (await launchdPrint(name)).text;
  const r = await sh('systemctl', [...(scope === 'user' ? ['--user'] : []), 'status', '--no-pager', '--lines=0', name]);
  return (r.out + r.err).trim();
}

// ---------- launchd (macOS) ----------
// Only the user's GUI domain: listing system daemons needs root, like system units on Linux.

const guiDomain = () => `gui/${process.getuid?.() ?? 501}`;

/** `launchctl list`: "PID\tStatus\tLabel" rows; PID "-" — not running, Status — last exit code. */
export function parseLaunchctlList(out: string): UnitInfo[] {
  return out
    .split('\n')
    .slice(1)
    .map((l) => l.split('\t'))
    .filter((c) => c.length >= 3 && c[2])
    .map(([pid, status, label]) => {
      const running = pid !== '-';
      const failed = !running && status !== '0' && status !== '-';
      return {
        name: label,
        scope: 'user' as const,
        load: 'loaded',
        active: running ? 'active' : failed ? 'failed' : 'inactive',
        sub: running ? `running (pid ${pid})` : failed ? `exited ${status}` : 'idle',
        description: label.startsWith('com.apple.') ? 'Apple' : '',
      };
    });
}

async function launchdAgents(): Promise<UnitInfo[]> {
  const r = await sh('launchctl', ['list']);
  return r.code === 0 ? parseLaunchctlList(r.out) : [];
}

async function launchdAction(label: string, action: 'start' | 'stop' | 'restart') {
  const target = `${guiDomain()}/${label}`;
  const args = action === 'stop' ? ['kill', 'SIGTERM', target] : action === 'restart' ? ['kickstart', '-k', target] : ['kickstart', target];
  const r = await sh('launchctl', args, 60_000);
  return { ok: r.code === 0, output: (r.out + r.err).trim() || 'OK' };
}

async function launchdPrint(label: string): Promise<{ text: string; out: string | null; err: string | null }> {
  const r = await sh('launchctl', ['print', `${guiDomain()}/${label}`]);
  const text = (r.out + r.err).trim();
  const path = (k: string) => text.match(new RegExp(`${k} = (.+)`))?.[1]?.trim() ?? null;
  return { text, out: path('stdout path'), err: path('stderr path') };
}

/** Agents log to files from their plist (StandardOutPath/ErrorPath); otherwise to the unified log. */
async function launchdLogs(label: string, lines: number): Promise<string> {
  const p = await launchdPrint(label);
  const files = [...new Set([p.out, p.err].filter((f): f is string => !!f && f !== '/dev/null'))];
  if (files.length) {
    const parts = await Promise.all(files.map(async (f) => `==> ${f} <==\n${(await sh('tail', ['-n', String(lines), f])).out.trim()}`));
    return parts.join('\n\n');
  }
  const proc = label.split('.').pop()!;
  const r = await sh('log', ['show', '--last', '1h', '--style', 'compact', '--predicate', `process == "${proc}"`], 30_000);
  return tail(r.out + r.err, lines) || 'No log files configured for this agent and nothing in the unified log for the last hour.';
}

// ---------- packages: details of a pacman package (lists are in packages.ts) ----------

const pkgName = (name: string) => (/^[\w@.+-]+$/.test(name) ? name : null);

export async function packageInfo(name: string): Promise<string> {
  if (!pkgName(name)) return 'Invalid package name';
  const r = await sh('pacman', ['-Qi', name]);
  let log = '';
  try {
    log = readFileSync('/var/log/pacman.log', 'utf8')
      .split('\n')
      .filter((l) => l.includes(` ${name} (`))
      .slice(-10)
      .join('\n');
  } catch {}
  return `${(r.out + r.err).trim()}\n\nRecent pacman.log entries:\n${log || '—'}`;
}

export { tail };
