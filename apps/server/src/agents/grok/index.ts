import { shq } from '../../terminal.ts';
import { action, cachedLatest, configIn, notFound, parseVersion, runCli, versionInfo } from '../kit.ts';
import type { AgentModule } from '../types.ts';
import { grokExtensions, grokMcp } from './extensions.ts';
import { GROK_BIN, grokAdapter, grokDir, grokLoggedIn, grokModels, grokVersion } from './adapter.ts';

const KIND = 'grok';

export const grokModule: AgentModule = {
  kind: KIND,
  label: 'Grok CLI',
  adapter: grokAdapter,
  modelStyle: 'search',
  async status() {
    const v = await grokVersion();
    const signedIn = grokLoggedIn();
    return {
      available: Boolean(v) && signedIn,
      detail: !v ? notFound(GROK_BIN) : signedIn ? `CLI ${v}` : 'Not signed in — see Harness',
      version: parseVersion(v),
      signedIn: v ? signedIn : null,
    };
  },
  async models() {
    return { models: grokModels() };
  },
  async harness() {
    const installed = await grokVersion();
    const loggedIn = Boolean(installed) && grokLoggedIn();
    const latest = installed
      ? await cachedLatest(KIND, 'self', async () => {
          const out = await runCli(GROK_BIN, ['update', '--check', '--json'], 30_000);
          return parseVersion(JSON.parse(out ?? '{}').latestVersion);
        })
      : null;
    const bin = shq(GROK_BIN);
    return {
      kind: KIND,
      label: 'Grok CLI',
      protocol: 'acp',
      pinned: false,
      version: versionInfo(installed, latest, installed ? 'grok installer' : '—', installed ? `${bin} update` : null),
      actions: installed
        ? [action('update', `${bin} update`), action('login', `${bin} login --device-auth`), action('logout', `${bin} logout`)]
        : [action('install', 'curl -fsSL https://x.ai/cli/install.sh | bash')],
      accounts: installed ? [{ id: 'grok', label: 'grok.com', detail: loggedIn ? 'session' : null, loggedIn, active: true }] : [],
      auth: { loggedIn, method: loggedIn ? 'grok.com account' : null },
      accountMode: 'single',
      limits: null,
      limitsAsOf: null,
      models: grokModels().map((id) => ({ id, label: id })),
      defaultModel: null,
      defaultModelEdit: null,
      usage: null,
    };
  },
  statePaths: () => [grokDir],
  // MCP servers, instructions, skills, subagents, commands
  configPaths: () => configIn(grokDir, ['config.toml', 'AGENTS.md'], ['skills', 'agents', 'commands']),
  mcp: grokMcp,
  extensions: grokExtensions,
};
