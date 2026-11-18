import { existsSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import type { HarnessAccount, HarnessInfo } from '@hubness/shared';
import { HttpError } from '../../errors.ts';
import { runOnce, shq } from '../../terminal.ts';
import { action, cachedLatest, cmpVersion, notFound, parseVersion, readJson, resolveBin, runCli, versionInfo, type Json } from '../kit.ts';
import type { AgentModule } from '../types.ts';
import { OPENCODE_BIN, opencodeAdapter, opencodeModels, opencodeVersion } from './adapter.ts';

/**
 * Opencode harness: version (pacman or GitHub releases), provider credentials
 * (`opencode auth list`), models of connected providers, default model, usage stats.
 */

const KIND = 'opencode';

async function opencodeSource(real: string | null): Promise<{ latest: string | null; source: string; pacman: boolean }> {
  // installed by a package manager — check against its repository
  const owner = real ? await runCli('pacman', ['-Qqo', real], 5_000) : null;
  if (owner?.trim()) {
    const pkg = owner.trim().split('\n')[0]!;
    const latest = await cachedLatest(KIND, `pacman:${pkg}`, async () => {
      const si = await runCli('pacman', ['-Si', pkg], 10_000);
      return parseVersion(si?.match(/^Version\s*:\s*(\S+)/m)?.[1]);
    });
    return { latest, source: 'pacman', pacman: true };
  }
  const latest = await cachedLatest(KIND, 'github', async () => {
    const res = await fetch('https://api.github.com/repos/anomalyco/opencode/releases?per_page=10', {
      signal: AbortSignal.timeout(10_000),
      headers: { Accept: 'application/vnd.github+json' },
    });
    if (!res.ok) return null;
    const versions = ((await res.json()) as Json[]).map((r) => parseVersion(r.tag_name)).filter(Boolean) as string[];
    return versions.sort(cmpVersion).pop() ?? null;
  });
  return { latest, source: 'GitHub releases', pacman: false };
}

/**
 * Update opencode by install method. `opencode upgrade` detects the
 * method only for its own installers; a manually placed binary is updated
 * in place with a fresh archive from GitHub releases.
 */
function opencodeUpgradeCommand(real: string | null, latest: string | null): string {
  const bin = shq(OPENCODE_BIN);
  const v = latest ? ` ${latest}` : '';
  if (!real) return `${bin} upgrade${v}`;
  if (real.includes('/.opencode/bin/')) return `${bin} upgrade${v} --method curl`;
  if (real.includes('/.bun/')) return `${bin} upgrade${v} --method bun`;
  if (real.includes('/pnpm/')) return `${bin} upgrade${v} --method pnpm`;
  if (real.includes('/node_modules/')) return `${bin} upgrade${v} --method npm`;
  if (real.includes('brew')) return `${bin} upgrade${v} --method brew`;
  // GitHub marks "latest" on the 1.x branch — download an explicit version
  const tag = latest ? `download/v${latest}` : 'latest/download';
  const arch = process.arch === 'arm64' ? 'arm64' : 'x64';
  const dir = shq(real.slice(0, real.lastIndexOf('/')) || '/');
  return `curl -fsSL https://github.com/anomalyco/opencode/releases/${tag}/opencode-linux-${arch}.tar.gz | tar -xz -C ${dir} opencode`;
}

interface OcProvider {
  id: string;
  name: string;
  connections: { id: string; label: string; method: string; active?: boolean }[];
}

async function opencodeProviders(): Promise<OcProvider[]> {
  const out = await runCli(OPENCODE_BIN, ['auth', 'list', '--format', 'json'], 15_000);
  try {
    const list = JSON.parse(out ?? '[]') as Json[];
    return list.map((p) => ({
      id: String(p.id),
      name: String(p.name ?? p.id),
      connections: ((p.connections ?? []) as Json[]).map((c) => ({
        id: String(c.id),
        label: String(c.label ?? c.method ?? c.id),
        method: String(c.method ?? ''),
        active: typeof c.active === 'boolean' ? c.active : undefined,
      })),
    }));
  } catch {
    return [];
  }
}

function parseStats(out: string | null): HarnessInfo['usage'] {
  if (!out) return null;
  const clean = out.replace(/\x1b\[[0-9;]*m/g, '');
  const num = (re: RegExp) => {
    const m = clean.match(re);
    return m ? Number(m[1]!.replace(/,/g, '')) : null;
  };
  const tokens = clean.match(/([\d.,]+[kKmMbB]?)\s+tokens/)?.[1] ?? null;
  const sessions = num(/([\d,]+)\s+sessions?/);
  const prompts = num(/([\d,]+)\s+prompts?/);
  return sessions === null && prompts === null && tokens === null ? null : { sessions, prompts, tokens };
}

const ocConfigDir = () => join(homedir(), '.config', 'opencode');

function opencodeDefaultModel(): string | null {
  for (const f of ['opencode.json', 'opencode.jsonc']) {
    const cfg = readJson(join(ocConfigDir(), f));
    if (cfg?.model) return String(cfg.model);
  }
  const state = readJson(join(homedir(), '.local', 'state', 'opencode', 'model.json'));
  const r = state?.recent?.[0];
  return r ? `${r.providerID}/${r.modelID}` : null;
}

/** Default model — the model key in ~/.config/opencode/opencode.json (the rest of the config is untouched). */
export function setOpencodeDefaultModel(model: string | null) {
  if (model !== null && !/^[\w.~-]+\/\S+$/.test(model)) throw new HttpError(400, 'Model must look like provider/model');
  const path = join(ocConfigDir(), 'opencode.json');
  let cfg: Json = {};
  if (existsSync(path)) {
    const parsed = readJson(path);
    if (!parsed) throw new HttpError(409, 'opencode.json is not plain JSON — edit it by hand');
    cfg = parsed;
  } else cfg = { $schema: 'https://opencode.ai/config.json' };
  if (model) cfg.model = model;
  else delete cfg.model;
  writeFileSync(path, JSON.stringify(cfg, null, 2) + '\n');
}

/** Models of connected providers only: the others won't run anyway. */
async function connectedModels(providers?: OcProvider[]): Promise<string[]> {
  const list = providers ?? (await opencodeProviders());
  const all = await opencodeModels();
  if (!list.length) return all;
  const ids = new Set(list.filter((p) => p.connections.length).map((p) => p.id));
  const cfg = readJson(join(ocConfigDir(), 'opencode.json'));
  for (const id of Object.keys(cfg?.provider ?? {})) ids.add(id);
  return all.filter((m) => ids.has(m.split('/')[0]!));
}

async function opencodeHarness(): Promise<HarnessInfo> {
  const real = await resolveBin(OPENCODE_BIN);
  const installed = parseVersion(await runCli(OPENCODE_BIN, ['--version']));
  const { latest, source, pacman } = await opencodeSource(real);
  const pkgUpdate = pacman ? 'sudo pacman -Syu opencode' : opencodeUpgradeCommand(real, latest);
  const version = versionInfo(installed, latest, installed ? source : '—', pkgUpdate);
  const base = {
    kind: KIND,
    label: 'Opencode',
    protocol: 'acp' as const,
    pinned: false,
    version,
    auth: null,
    accountMode: 'providers' as const,
    limits: null,
    limitsAsOf: null,
    defaultModelEdit: 'search' as const,
  };
  if (!installed) {
    return {
      ...base,
      actions: [action('install', 'curl -fsSL https://opencode.ai/install | bash'), action('install', 'sudo pacman -S opencode', { runnable: false })],
      accounts: [],
      defaultModel: null,
      models: [],
      usage: null,
    };
  }
  // opencode commands run one at a time: in parallel the CLI sometimes returns empty output
  const providers = await opencodeProviders();
  const statsOut = providers.length ? await runCli(OPENCODE_BIN, ['stats'], 15_000) : null;
  const models = await connectedModels(providers);
  const accounts: HarnessAccount[] = providers.flatMap((p) =>
    p.connections.map((c) => ({
      id: c.id,
      label: c.label,
      detail: c.method || null,
      loggedIn: true,
      active: c.active ?? p.connections.length === 1,
      provider: p.id,
      providerName: p.name,
    })),
  );
  return {
    ...base,
    actions: [action('update', pkgUpdate, { runnable: !pacman }), action('login', `${shq(OPENCODE_BIN)} auth login`)],
    accounts,
    defaultModel: opencodeDefaultModel(),
    models: models.map((id) => ({ id, label: id })),
    usage: parseStats(statsOut),
  };
}

export const opencodeModule: AgentModule = {
  kind: KIND,
  label: 'Opencode',
  adapter: opencodeAdapter,
  modelStyle: 'search',
  async status() {
    const v = await opencodeVersion();
    return { available: Boolean(v), detail: v ? `CLI ${v}` : notFound(OPENCODE_BIN), version: parseVersion(v), signedIn: null };
  },
  async models() {
    return { models: await connectedModels() };
  },
  harness: () => opencodeHarness(),
  setDefaultModel: setOpencodeDefaultModel,
  async accountOp(provider, account, op) {
    const r = await runOnce(OPENCODE_BIN, ['auth', op, provider, account]);
    if (r.code !== 0) throw new HttpError(500, r.output || `opencode auth ${op} failed`);
    return r.output;
  },
  statePaths: () => ['.config/opencode', '.local/share/opencode', '.local/state/opencode'].map((d) => join(homedir(), d)),
};
