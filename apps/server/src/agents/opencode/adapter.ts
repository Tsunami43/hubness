import type { PermissionMode } from '@hubness/shared';
import '../../config.ts';
import { createAcpAdapter } from '../acp.ts';
import { runCli } from '../kit.ts';

/** Opencode CLI binary. */
export const OPENCODE_BIN = process.env.HUBNESS_OPENCODE_BIN ?? 'opencode';

/**
 * Opencode via ACP (`opencode acp`). Permission mode and hub MCP servers
 * are passed via inline config (OPENCODE_CONFIG_CONTENT): opencode ignores the ACP
 * mcpServers parameter. Tested on opencode 2.0.23.
 */

type Json = Record<string, any>;

function permissionConfig(mode: PermissionMode): Json {
  switch (mode) {
    case 'bypass':
      return { permission: { edit: 'allow', bash: 'allow', webfetch: 'allow' } };
    case 'acceptEdits':
      return { permission: { edit: 'allow', bash: 'ask', webfetch: 'ask' } };
    case 'plan':
      return { permission: { edit: 'deny', bash: 'ask', webfetch: 'ask' } };
    default:
      return { permission: { edit: 'ask', bash: 'ask', webfetch: 'ask' } };
  }
}

/**
 * Opencode reports retries after a provider error in _meta['opencode/retry']:
 * attempt number, time of the next one, error (status, response body). We extract
 * the real cause: for OpenRouter it is in error.metadata.raw.
 */
export function describeRetry(u: Record<string, any>): { retry: string; error: string } | null {
  const r = u?._meta?.['opencode/retry'];
  if (!r?.error) return null;
  const e = r.error;
  let reason = e.message ?? 'Provider error';
  try {
    const body = JSON.parse(e.response?.body ?? '');
    reason = body?.error?.metadata?.raw ?? body?.error?.message ?? reason;
  } catch {}
  const status = e.status ? `${e.status} ` : '';
  const when = r.nextRetryAt ? Math.max(0, Math.round((Date.parse(r.nextRetryAt) - Date.now()) / 1000)) : null;
  const error = `${status}${reason}`.trim();
  return { error, retry: `Provider error ${error} — retry #${r.attempt}${when !== null ? ` in ${when}s` : ''}` };
}

export const opencodeAdapter = createAcpAdapter({
  kind: 'opencode',
  label: 'Opencode',
  mcpViaParam: false,
  nativePermissions: true,
  describeInfo: describeRetry,
  command(ctx, mode) {
    const inlineConfig = {
      ...permissionConfig(mode),
      mcp: Object.fromEntries(
        ctx.mcpServers.map((m) => [m.name, { type: 'local', command: [m.command, ...m.args], environment: m.env, enabled: true }]),
      ),
    };
    return { cmd: OPENCODE_BIN, args: ['acp'], env: { OPENCODE_CONFIG_CONTENT: JSON.stringify(inlineConfig) } };
  },
  async afterSession(call, sessionId, mode) {
    await call('session/set_config_option', { sessionId, configId: 'mode', value: mode === 'plan' ? 'plan' : 'build' }).catch(() => {});
  },
});


export async function opencodeVersion(): Promise<string | null> {
  return (await runCli(OPENCODE_BIN, ['--version'], 10_000))?.trim() || null;
}

let modelsCache: { at: number; models: string[] } | null = null;

/** Models available in opencode (`opencode models`), cached for 10 minutes. */
export async function opencodeModels(): Promise<string[]> {
  if (modelsCache && Date.now() - modelsCache.at < 600_000) return Promise.resolve(modelsCache.models);
  // with parallel calls opencode sometimes returns an empty list: retry and don't cache empty results
  for (let attempt = 0; attempt < 3; attempt++) {
    const out = await runCli(OPENCODE_BIN, ['models'], 30_000);
    if (out === null) break; // error or timeout: retrying won't help
    const models = (out ?? '')
      .split('\n')
      .map((l) => l.trim())
      .filter((l) => /^[\w.~-]+\/\S+$/.test(l));
    if (models.length) {
      modelsCache = { at: Date.now(), models };
      return models;
    }
    await new Promise((r) => setTimeout(r, 400));
  }
  return modelsCache?.models ?? [];
}
