import { execFileSync } from 'node:child_process';
import { existsSync, readdirSync, readFileSync, realpathSync } from 'node:fs';
import { join } from 'node:path';
import { arch, cpus, release, type as osType } from 'node:os';
import type { GpuInfo, HostInfo, HostPlatform, SystemPlatform, TempSensor } from '@hubness/shared';
import { childEnv } from '../config.ts';
import { onPath, sh } from './exec.ts';

/**
 * Host facts and sensors for System → Overview and platform detection for the
 * System pages. Linux distributions and macOS are supported; anything else gets the
 * portable basics from node:os only.
 */

export const PLATFORM: HostPlatform = process.platform === 'linux' ? 'linux' : process.platform === 'darwin' ? 'darwin' : 'other';

const read = (path: string) => {
  try {
    return readFileSync(path, 'utf8').trim();
  } catch {
    return '';
  }
};

/** Synchronous one-liner for rare static facts (sysctl, sw_vers). */
const run = (cmd: string, args: string[]) => {
  try {
    return execFileSync(cmd, args, { encoding: 'utf8', timeout: 5000, stdio: ['ignore', 'pipe', 'ignore'], env: childEnv() }).trim();
  } catch {
    return '';
  }
};

let platformCache: SystemPlatform | null = null;

/** systemd only when it is PID 1 (containers and OpenRC/runit hosts have systemctl without it). */
export function systemPlatform(): SystemPlatform {
  if (platformCache) return platformCache;
  let services: SystemPlatform['services'] = null;
  if (PLATFORM === 'linux' && existsSync('/run/systemd/system') && onPath('systemctl')) services = 'systemd';
  if (PLATFORM === 'darwin' && onPath('launchctl')) services = 'launchd';
  return (platformCache = { platform: PLATFORM, services });
}

export function parseOsRelease(text: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const line of text.split('\n')) {
    const m = line.match(/^([A-Z_]+)=(.*)$/);
    if (m) out[m[1]] = m[2].replace(/^["']|["']$/g, '');
  }
  return out;
}

function osFacts(): Pick<HostInfo, 'osId' | 'osLike' | 'osName'> {
  if (PLATFORM === 'darwin') {
    const name = run('sw_vers', ['-productName']) || 'macOS';
    const version = run('sw_vers', ['-productVersion']);
    return { osId: 'macos', osLike: [], osName: [name, version].filter(Boolean).join(' ') };
  }
  if (PLATFORM === 'linux') {
    const r = parseOsRelease(read('/etc/os-release') || read('/usr/lib/os-release'));
    return {
      osId: (r.ID ?? 'linux').toLowerCase(),
      osLike: (r.ID_LIKE ?? '').toLowerCase().split(/\s+/).filter(Boolean),
      osName: r.PRETTY_NAME || r.NAME || 'Linux',
    };
  }
  return { osId: process.platform, osLike: [], osName: osType() };
}

/** Vendor junk the firmware leaves in DMI fields. */
const DMI_JUNK = /^(to be filled.*|default string|system product name|system version|not applicable|none|o\.e\.m\.?|x\.x)$/i;

function hostModel(): string | null {
  if (PLATFORM === 'darwin') return run('sysctl', ['-n', 'hw.model']) || null;
  if (PLATFORM !== 'linux') return null;
  const dmi = (f: string) => {
    const v = read(`/sys/devices/virtual/dmi/id/${f}`);
    return v && !DMI_JUNK.test(v) ? v : '';
  };
  // Raspberry Pi and other boards expose the model via device tree
  const dt = read('/sys/firmware/devicetree/base/model').replace(/\0/g, '');
  const name = dmi('product_name') || dt;
  if (!name) return null;
  const version = dmi('product_version');
  return version && !name.includes(version) ? `${name} (${version})` : name;
}

function cpuModel(): string | null {
  const raw = (PLATFORM === 'darwin' ? run('sysctl', ['-n', 'machdep.cpu.brand_string']) : '') || cpus()[0]?.model || '';
  if (!raw) return null;
  const model = raw
    .replace(/\((R|TM)\)/gi, '')
    .replace(/\s+(CPU|Processor)\b/gi, '')
    .replace(/\s+\d+-Core\b/gi, '')
    .replace(/\s+w\/\s.*$/i, '')
    .replace(/\s+@\s.*$/, '')
    .replace(/\s+/g, ' ')
    .trim();
  return model;
}

/** `Advanced Micro Devices, Inc. [AMD/ATI]` + `Strix [Radeon 880M / 890M]` → `AMD Radeon 880M / 890M`. */
export function gpuName(vendor: string, device: string): string {
  const bracket = (s: string) => s.match(/\[([^\]]+)\]/)?.[1];
  const v = (bracket(vendor) ?? vendor).replace(/\/ATI$/, '').replace(/,? (Inc|Corporation|Corp)\.?$/i, '').trim();
  const d = bracket(device) ?? device;
  return d.toLowerCase().startsWith(v.toLowerCase()) ? d : `${v} ${d}`;
}

interface GpuDevice {
  /** PCI address `0000:c5:00.0` (lowercase) to match sensors; null on macOS. */
  addr: string | null;
  name: string;
}

async function gpuDevices(): Promise<GpuDevice[]> {
  if (PLATFORM === 'darwin') {
    const r = await sh('system_profiler', ['SPDisplaysDataType', '-json'], 15_000);
    try {
      return ((JSON.parse(r.out).SPDisplaysDataType ?? []) as { sppci_model?: string; _name?: string }[])
        .map((g) => ({ addr: null, name: g.sppci_model ?? g._name ?? '' }))
        .filter((g) => g.name);
    } catch {
      return [];
    }
  }
  if (PLATFORM !== 'linux' || !onPath('lspci')) return [];
  const r = await sh('lspci', ['-mm', '-D']);
  return r.out
    .split('\n')
    .map((l) => ({ addr: l.split(' ')[0]?.toLowerCase() ?? '', f: [...l.matchAll(/"([^"]*)"/g)].map((m) => m[1]) }))
    .filter(({ f }) => /VGA|3D|Display/i.test(f[0] ?? ''))
    .map(({ addr, f }) => ({ addr, name: gpuName(f[1] ?? '', f[2] ?? '') }));
}

interface Static {
  info: HostInfo;
  gpus: GpuDevice[];
}

const TTL_MS = 10 * 60_000;
let cache: { at: number; value: Promise<Static> } | null = null;

async function readStatic(): Promise<Static> {
  return {
    info: {
      ...systemPlatform(),
      ...osFacts(),
      arch: arch() === 'x64' ? 'x86_64' : arch() === 'arm64' ? (PLATFORM === 'darwin' ? 'arm64' : 'aarch64') : arch(),
      kernel: `${PLATFORM === 'darwin' ? 'Darwin' : osType()} ${release()}`,
      host: hostModel(),
      cpu: cpuModel(),
    },
    gpus: await gpuDevices(),
  };
}

/** Static facts are cached for 10 minutes (lspci / system_profiler are the slow part). */
function staticFacts(): Promise<Static> {
  if (!cache || Date.now() - cache.at > TTL_MS) {
    const value = readStatic();
    cache = { at: Date.now(), value };
    value.catch(() => (cache = null));
  }
  return cache.value;
}

export const hostInfo = async (): Promise<HostInfo> => (await staticFacts()).info;

// ---------- live sensors (Linux hwmon, nvidia-smi) ----------

const CPU_SENSORS = ['k10temp', 'coretemp', 'zenpower', 'cpu_thermal', 'soc_thermal'];
const GPU_SENSORS = ['amdgpu', 'radeon', 'nouveau', 'i915', 'xe'];
const DISK_SENSORS = ['nvme', 'drivetemp'];
/** Preferred input per chip: CPU package / die, GPU edge, NVMe composite. */
const PREFERRED = /^(tctl|tdie|package id 0|edge|composite)$/i;

interface HwmonChip {
  name: string;
  /** PCI address of the device behind the chip (GPUs). */
  addr: string | null;
  celsius: number | null;
  busy: number | null;
}

function hwmonChips(): HwmonChip[] {
  const base = '/sys/class/hwmon';
  let dirs: string[] = [];
  try {
    dirs = readdirSync(base);
  } catch {
    return [];
  }
  return dirs.map((d) => {
    const dir = join(base, d);
    const inputs = (() => {
      try {
        return readdirSync(dir).filter((f) => /^temp\d+_input$/.test(f));
      } catch {
        return [];
      }
    })()
      .map((f) => ({ label: read(join(dir, f.replace('_input', '_label'))), value: Number(read(join(dir, f))) }))
      .filter((t) => Number.isFinite(t.value) && t.value > 0);
    const pick = inputs.find((t) => PREFERRED.test(t.label)) ?? inputs[0];
    let addr: string | null = null;
    try {
      addr = realpathSync(join(dir, 'device')).split('/').pop()!.toLowerCase();
    } catch {}
    const busy = read(join(dir, 'device', 'gpu_busy_percent'));
    return { name: read(join(dir, 'name')), addr, celsius: pick ? Math.round(pick.value / 100) / 10 : null, busy: busy ? Number(busy) : null };
  });
}

interface NvidiaRow {
  addr: string;
  usage: number | null;
  temp: number | null;
}

async function nvidiaRows(): Promise<NvidiaRow[]> {
  if (!onPath('nvidia-smi')) return [];
  const r = await sh('nvidia-smi', ['--query-gpu=pci.bus_id,utilization.gpu,temperature.gpu', '--format=csv,noheader,nounits'], 5000);
  const num = (v: string | undefined) => (v && /^\d+(\.\d+)?$/.test(v.trim()) ? Number(v) : null);
  return r.out
    .split('\n')
    .filter(Boolean)
    .map((l) => l.split(',').map((c) => c.trim()))
    // nvidia-smi pads the domain to 8 digits: 00000000:01:00.0 → 0000:01:00.0
    .map(([bus, u, t]) => ({ addr: (bus ?? '').toLowerCase().replace(/^0{4}(?=[0-9a-f]{4}:)/, ''), usage: num(u), temp: num(t) }));
}

export async function sensors(): Promise<{ gpus: GpuInfo[]; temps: TempSensor[] }> {
  const { gpus } = await staticFacts();
  const chips = PLATFORM === 'linux' ? hwmonChips() : [];
  const nvidia = await nvidiaRows();
  const temps: TempSensor[] = [];
  const add = (kind: TempSensor['kind'], label: string, c: number | null) => {
    if (c !== null && c > 0 && c < 150) temps.push({ kind, label, celsius: c });
  };
  const cpu = chips.find((c) => CPU_SENSORS.includes(c.name) && c.celsius !== null) ?? chips.find((c) => c.name === 'acpitz');
  add('cpu', 'CPU', cpu?.celsius ?? null);
  const gpuInfo: GpuInfo[] = gpus.map((g) => {
    const chip = chips.find((c) => GPU_SENSORS.includes(c.name) && c.addr === g.addr);
    const nv = nvidia.find((n) => n.addr === g.addr);
    return { name: g.name, usagePercent: nv?.usage ?? chip?.busy ?? null, tempC: nv?.temp ?? chip?.celsius ?? null };
  });
  gpuInfo.forEach((g, i) => add('gpu', gpuInfo.length > 1 ? `GPU ${i + 1}` : 'GPU', g.tempC));
  chips
    .filter((c) => DISK_SENSORS.includes(c.name))
    .forEach((c, i, all) => {
      const name = c.name === 'nvme' ? 'NVMe' : 'Disk';
      add('disk', all.length > 1 ? `${name} ${i + 1}` : name, c.celsius);
    });
  return { gpus: gpuInfo, temps };
}
