import { existsSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import type { PackageInfo, PackageManagerInfo, PackagesResponse } from '@hubness/shared';
import { HttpError } from '../errors.ts';
import { onPath, sh } from './exec.ts';

/**
 * Package managers of the server (System → Packages): one tab per manager found on PATH.
 *
 * Detection is cheap (looks for the binaries). A manager's data is read only when its tab is
 * opened and cached for CACHE_MS — some checks go to the network (AUR, npm registry, rustup
 * dist server). «Refresh» bypasses the cache. Nothing here installs or updates anything.
 */

const CACHE_MS = 15 * 60_000;
const NET_TIMEOUT = 60_000;

interface Manager {
  id: string;
  label: string;
  kind: PackageManagerInfo['kind'];
  /** Binary that must be on PATH (the first found wins for alternatives). */
  bins: string[];
  read: (bin: string) => Promise<Omit<PackagesResponse, 'manager' | 'label' | 'checkedAt'>>;
}

const lines = (s: string) => s.split('\n').map((l) => l.trim()).filter(Boolean);

/** "name version" or "name version -> new" lines (pacman, yay -Qua). */
function nameVersion(out: string): PackageInfo[] {
  return lines(out)
    .map((l) => l.match(/^(\S+)\s+(\S+)(?:\s+->\s+(\S+))?/))
    .filter((m): m is RegExpMatchArray => Boolean(m))
    .map((m) => ({ name: m[1]!, version: m[2]!, newVersion: m[3] }));
}

function json<T>(s: string): T | null {
  try {
    return JSON.parse(s) as T;
  } catch {
    return null;
  }
}

const MANAGERS: Manager[] = [
  // ---------- system ----------
  {
    id: 'pacman',
    label: 'pacman',
    kind: 'system',
    bins: ['pacman'],
    async read() {
      const [all, explicit, upd] = await Promise.all([sh('pacman', ['-Q']), sh('pacman', ['-Qen']), sh('pacman', ['-Qu'])]);
      return {
        installed: nameVersion(all.out).length,
        packages: nameVersion(explicit.out),
        listTitle: 'explicit',
        updates: nameVersion(upd.out),
        updatesNote: 'From the last repository sync (pacman -Qu). `sudo pacman -Sy` refreshes it.',
      };
    },
  },
  {
    id: 'aur',
    label: 'AUR',
    kind: 'system',
    bins: ['paru', 'yay'],
    async read(bin) {
      const [foreign, upd] = await Promise.all([sh('pacman', ['-Qm']), sh(bin, ['-Qua'], NET_TIMEOUT)]);
      const list = nameVersion(foreign.out);
      return {
        installed: list.length,
        packages: list,
        listTitle: 'installed',
        updates: upd.code <= 1 ? nameVersion(upd.out) : null,
        updatesNote: `Checked against the AUR by ${bin} -Qua.`,
      };
    },
  },
  {
    id: 'apt',
    label: 'apt',
    kind: 'system',
    bins: ['apt'],
    async read() {
      const [all, manual, upd] = await Promise.all([
        sh('dpkg-query', ['-W', '-f=${Package} ${Version}\n']),
        sh('apt-mark', ['showmanual']),
        sh('apt', ['list', '--upgradable']),
      ]);
      const versions = new Map(nameVersion(all.out).map((p) => [p.name, p.version]));
      const updates = lines(upd.out)
        .map((l) => l.match(/^([^/\s]+)\/\S+\s+(\S+)\s+\S+\s+\[upgradable from: ([^\]]+)\]/))
        .filter((m): m is RegExpMatchArray => Boolean(m))
        .map((m) => ({ name: m[1]!, version: m[3]!, newVersion: m[2]! }));
      return {
        installed: versions.size,
        packages: lines(manual.out).map((n) => ({ name: n, version: versions.get(n) ?? '' })),
        listTitle: 'explicit',
        updates,
        updatesNote: 'From the last `apt update` (apt list --upgradable).',
      };
    },
  },
  {
    id: 'dnf',
    label: 'dnf',
    kind: 'system',
    bins: ['dnf'],
    async read() {
      const [all, user, upd] = await Promise.all([
        sh('rpm', ['-qa', '--qf', '%{NAME} %{VERSION}-%{RELEASE}\n']),
        sh('dnf', ['repoquery', '--userinstalled', '--qf', '%{name} %{evr}\n']),
        sh('dnf', ['check-update', '-q'], NET_TIMEOUT),
      ]);
      const versions = new Map(nameVersion(all.out).map((p) => [p.name, p.version]));
      const updates = lines(upd.out)
        .map((l) => l.match(/^(\S+?)\.\S+\s+(\S+)\s+\S+$/))
        .filter((m): m is RegExpMatchArray => Boolean(m))
        .map((m) => ({ name: m[1]!, version: versions.get(m[1]!) ?? '', newVersion: m[2]! }));
      return {
        installed: versions.size,
        packages: nameVersion(user.out),
        listTitle: 'explicit',
        // check-update exits 100 when there are updates
        updates: upd.code === 0 || upd.code === 100 ? updates : null,
        updatesNote: 'dnf check-update (uses the cached metadata when fresh).',
      };
    },
  },
  {
    id: 'flatpak',
    label: 'Flatpak',
    kind: 'system',
    bins: ['flatpak'],
    async read() {
      const [list, upd] = await Promise.all([
        sh('flatpak', ['list', '--app', '--columns=application,version']),
        sh('flatpak', ['remote-ls', '--updates', '--app', '--columns=application,version'], NET_TIMEOUT),
      ]);
      const parse = (s: string) => lines(s).map((l) => l.split('\t')).map(([name, version]) => ({ name: name!, version: version ?? '' }));
      const apps = parse(list.out);
      const current = new Map(apps.map((a) => [a.name, a.version]));
      return {
        installed: apps.length,
        packages: apps,
        listTitle: 'installed',
        updates: upd.code === 0 ? parse(upd.out).map((u) => ({ name: u.name, version: current.get(u.name) ?? '', newVersion: u.version })) : null,
        updatesNote: 'flatpak remote-ls --updates.',
      };
    },
  },
  {
    id: 'snap',
    label: 'Snap',
    kind: 'system',
    bins: ['snap'],
    async read() {
      const [list, upd] = await Promise.all([sh('snap', ['list']), sh('snap', ['refresh', '--list'], NET_TIMEOUT)]);
      const parse = (s: string) =>
        lines(s)
          .slice(1)
          .map((l) => l.split(/\s+/))
          .map(([name, version]) => ({ name: name!, version: version ?? '' }));
      const snaps = parse(list.out);
      const current = new Map(snaps.map((a) => [a.name, a.version]));
      return {
        installed: snaps.length,
        packages: snaps,
        listTitle: 'installed',
        updates: upd.code === 0 ? parse(upd.out).map((u) => ({ name: u.name, version: current.get(u.name) ?? '', newVersion: u.version })) : null,
        updatesNote: 'snap refresh --list.',
      };
    },
  },
  {
    id: 'brew',
    label: 'Homebrew',
    kind: 'system',
    bins: ['brew'],
    async read() {
      const [list, upd] = await Promise.all([sh('brew', ['list', '--versions']), sh('brew', ['outdated', '--verbose'], NET_TIMEOUT)]);
      const pkgs = lines(list.out).map((l) => l.split(/\s+/)).map(([name, ...v]) => ({ name: name!, version: v.join(' ') }));
      const updates = lines(upd.out)
        .map((l) => l.match(/^(\S+) \((.+)\) [<!=]+ (\S+)/))
        .filter((m): m is RegExpMatchArray => Boolean(m))
        .map((m) => ({ name: m[1]!, version: m[2]!, newVersion: m[3]! }));
      return { installed: pkgs.length, packages: pkgs, listTitle: 'installed', updates: upd.code === 0 ? updates : null, updatesNote: 'brew outdated.' };
    },
  },
  // ---------- languages ----------
  {
    id: 'cargo',
    label: 'cargo',
    kind: 'language',
    bins: ['cargo'],
    async read() {
      const r = await sh('cargo', ['install', '--list']);
      // "name v1.2.3:" or "name v1.2.3 (/local/path):", followed by indented binary names
      const pkgs = r.out
        .split('\n')
        .map((l) => l.match(/^(\S+) v(\S+?)(?: \(([^)]+)\))?:$/))
        .filter((m): m is RegExpMatchArray => Boolean(m))
        .map((m) => ({ name: m[1]!, version: m[2]!, note: m[3] }));
      const hasUpdater = onPath('cargo-install-update');
      let updates: PackageInfo[] | null = null;
      if (hasUpdater) {
        const u = await sh('cargo', ['install-update', '--list'], NET_TIMEOUT);
        updates = lines(u.out)
          .map((l) => l.match(/^(\S+)\s+v(\S+)\s+v(\S+)\s+Yes$/))
          .filter((m): m is RegExpMatchArray => Boolean(m))
          .map((m) => ({ name: m[1]!, version: m[2]!, newVersion: m[3]! }));
      }
      return {
        installed: pkgs.length,
        packages: pkgs,
        listTitle: 'installed',
        updates,
        updatesNote: hasUpdater ? 'cargo install-update --list.' : 'Install cargo-update (`cargo install cargo-update`) to see updates of cargo binaries.',
      };
    },
  },
  {
    id: 'rustup',
    label: 'rustup',
    kind: 'language',
    bins: ['rustup'],
    async read() {
      const [list, check] = await Promise.all([sh('rustup', ['toolchain', 'list']), sh('rustup', ['check'], NET_TIMEOUT)]);
      const toolchains = lines(list.out).map((l) => {
        const m = l.match(/^(\S+)(?: \((.+)\))?/)!;
        return { name: m[1]!, version: '', note: m[2] };
      });
      const updates = lines(check.out)
        .map((l) => l.match(/^(\S+) - update available\s*: (\S+).*? -> (\S+)/))
        .filter((m): m is RegExpMatchArray => Boolean(m))
        .map((m) => ({ name: m[1]!, version: m[2]!, newVersion: m[3]! }));
      return { installed: toolchains.length, packages: toolchains, listTitle: 'installed', updates: check.code <= 100 ? updates : null, updatesNote: 'rustup check.' };
    },
  },
  {
    id: 'npm',
    label: 'npm (global)',
    kind: 'language',
    bins: ['npm'],
    async read() {
      const [ls, outdated] = await Promise.all([sh('npm', ['ls', '-g', '--depth=0', '--json']), sh('npm', ['outdated', '-g', '--json'], NET_TIMEOUT)]);
      const deps = json<{ dependencies?: Record<string, { version?: string }> }>(ls.out)?.dependencies ?? {};
      const pkgs = Object.entries(deps).map(([name, d]) => ({ name, version: d.version ?? '' }));
      const old = json<Record<string, { current?: string; latest?: string }>>(outdated.out || '{}');
      return {
        installed: pkgs.length,
        packages: pkgs,
        listTitle: 'installed',
        updates: old ? Object.entries(old).map(([name, o]) => ({ name, version: o.current ?? '', newVersion: o.latest })) : null,
        updatesNote: 'npm outdated -g.',
      };
    },
  },
  {
    id: 'uv',
    label: 'uv tools',
    kind: 'tool',
    bins: ['uv'],
    async read() {
      const [list, outdated] = await Promise.all([sh('uv', ['tool', 'list']), sh('uv', ['tool', 'list', '--outdated'], NET_TIMEOUT)]);
      const parse = (s: string) =>
        lines(s)
          .filter((l) => !l.startsWith('-'))
          .map((l) => l.match(/^(\S+) v(\S+)(?: \[latest: v?(\S+)\])?/))
          .filter((m): m is RegExpMatchArray => Boolean(m))
          .map((m) => ({ name: m[1]!, version: m[2]!, newVersion: m[3] }));
      const tools = parse(list.out);
      return {
        installed: tools.length,
        packages: tools,
        listTitle: 'installed',
        updates: outdated.code === 0 ? parse(outdated.out).filter((p) => p.newVersion) : null,
        updatesNote: 'uv tool list --outdated.',
      };
    },
  },
  {
    id: 'pipx',
    label: 'pipx',
    kind: 'tool',
    bins: ['pipx'],
    async read() {
      const r = await sh('pipx', ['list', '--json']);
      const venvs = json<{ venvs?: Record<string, { metadata?: { main_package?: { package_version?: string } } }> }>(r.out)?.venvs ?? {};
      const pkgs = Object.entries(venvs).map(([name, v]) => ({ name, version: v.metadata?.main_package?.package_version ?? '' }));
      return { installed: pkgs.length, packages: pkgs, listTitle: 'installed', updates: null, updatesNote: 'pipx has no update check; `pipx upgrade-all` upgrades everything.' };
    },
  },
  {
    id: 'pip',
    label: 'pip (user)',
    kind: 'language',
    bins: ['pip'],
    async read() {
      const [list, outdated] = await Promise.all([sh('pip', ['list', '--user', '--format=json']), sh('pip', ['list', '--user', '--outdated', '--format=json'], NET_TIMEOUT)]);
      const pkgs = json<{ name: string; version: string }[]>(list.out) ?? [];
      const old = json<{ name: string; version: string; latest_version: string }[]>(outdated.out);
      return {
        installed: pkgs.length,
        packages: pkgs,
        listTitle: 'installed',
        updates: old ? old.map((o) => ({ name: o.name, version: o.version, newVersion: o.latest_version })) : null,
        updatesNote: 'pip list --user --outdated. System Python packages belong to the system package manager.',
      };
    },
  },
  {
    id: 'go',
    label: 'go install',
    kind: 'language',
    bins: ['go'],
    async read() {
      const gopath = (await sh('go', ['env', 'GOPATH'])).out.trim();
      const gobin = (await sh('go', ['env', 'GOBIN'])).out.trim() || (gopath ? join(gopath, 'bin') : '');
      const files = gobin && existsSync(gobin) ? readdirSync(gobin).slice(0, 200) : [];
      const pkgs = await Promise.all(
        files.map(async (f) => {
          const r = await sh('go', ['version', '-m', join(gobin, f)]);
          const mod = r.out.match(/^\s*mod\s+(\S+)\s+(\S+)/m);
          return { name: f, version: mod?.[2] ?? '', note: mod?.[1] };
        }),
      );
      return { installed: pkgs.length, packages: pkgs, listTitle: 'installed', updates: null, updatesNote: `Binaries in ${gobin || 'GOBIN'}; reinstall with \`go install <module>@latest\` to update.` };
    },
  },
];

// ---------- detection and cache ----------

let detected: { at: number; list: (PackageManagerInfo & { bin: string })[] } | null = null;

function detect() {
  if (detected && Date.now() - detected.at < CACHE_MS) return detected.list;
  const list = MANAGERS.flatMap((m) => {
    const bin = m.bins.find(onPath);
    return bin ? [{ id: m.id, label: m.label, kind: m.kind, bin }] : [];
  });
  detected = { at: Date.now(), list };
  return list;
}

export const packageManagers = (): PackageManagerInfo[] => detect().map(({ bin: _bin, ...m }) => m);

const cache = new Map<string, { at: number; data: Promise<PackagesResponse> }>();

export function managerPackages(id: string, refresh = false): Promise<PackagesResponse> {
  const found = detect().find((m) => m.id === id);
  const m = MANAGERS.find((x) => x.id === id);
  if (!found || !m) throw new HttpError(404, `Package manager “${id}” not found on this server`);
  const hit = cache.get(id);
  if (hit && !refresh && Date.now() - hit.at < CACHE_MS) return hit.data;
  const data = m.read(found.bin).then(
    (r) => ({ ...r, manager: id, label: found.label, checkedAt: new Date().toISOString() }),
    (e) => {
      cache.delete(id);
      throw e;
    },
  );
  cache.set(id, { at: Date.now(), data });
  return data;
}
