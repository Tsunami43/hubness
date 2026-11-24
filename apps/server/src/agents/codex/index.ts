import { action, cachedLatest, npmLatest, versionInfo } from '../kit.ts';
import type { AgentModule } from '../types.ts';
import { CODEX_ACP_PKG, codexAcpCommand, codexAdapter, codexAuth, codexDir, codexInstallCommand, codexInstalled, codexModels, npmGlobalWritable } from './adapter.ts';

const KIND = 'codex';

/** The adapter runs sessions, the CLI signs in: both are needed (a custom adapter command counts as installed). */
function ready() {
  const inst = codexInstalled();
  const custom = inst.acp === 'custom';
  return { inst, custom, installed: custom ? inst.acp : inst.acp && inst.cli ? inst.acp : null };
}

export const codexModule: AgentModule = {
  kind: KIND,
  label: 'Codex',
  adapter: codexAdapter,
  modelStyle: 'search',
  async status() {
    const { inst, custom, installed } = ready();
    const method = codexAuth();
    return {
      available: Boolean(installed) && Boolean(method),
      detail: !installed ? 'Not installed — see Harness' : method ? `codex-acp ${inst.acp}` : 'Not signed in — see Harness',
      version: installed && !custom ? inst.acp : null,
      signedIn: installed ? Boolean(method) : null,
    };
  },
  async models() {
    return { models: codexModels() };
  },
  async harness() {
    const { custom, installed } = ready();
    const writable = npmGlobalWritable();
    const latest = installed && !custom ? await cachedLatest(KIND, 'npm', () => npmLatest(CODEX_ACP_PKG)) : null;
    const update = latest ? codexInstallCommand(latest) : null;
    const method = installed ? codexAuth() : null;
    return {
      kind: KIND,
      label: 'Codex',
      protocol: 'acp',
      pinned: true,
      version: custom
        ? { installed: null, latest: null, status: 'unknown', source: `HUBNESS_CODEX_ACP (${codexAcpCommand().join(' ')})`, updateCommand: null }
        : versionInfo(installed, latest, installed ? 'npm -g (codex-acp)' : '—', update),
      actions: installed
        ? [
            ...(update && !custom ? [action('update', update, { runnable: writable })] : []),
            action('login', 'codex login --device-auth'),
            action('logout', 'codex logout'),
          ]
        : [action('install', codexInstallCommand(), { runnable: writable })],
      accounts: installed ? [{ id: 'codex', label: 'OpenAI', detail: method, loggedIn: Boolean(method), active: true }] : [],
      auth: { loggedIn: Boolean(method), method },
      accountMode: 'single',
      limits: null,
      limitsAsOf: null,
      models: codexModels().map((id) => ({ id, label: id })),
      defaultModel: null,
      defaultModelEdit: null,
      usage: null,
    };
  },
  statePaths: () => [codexDir],
};
