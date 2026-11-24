import { existsSync, readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import '../../config.ts';
import { kv } from '../../db/index.ts';
import { createAcpAdapter, modelsKey } from '../acp.ts';
import { runCli } from '../kit.ts';

/** Grok CLI binary. */
export const GROK_BIN = process.env.HUBNESS_GROK_BIN ?? 'grok';

/**
 * Grok CLI via ACP (`grok agent stdio`). The hub MCP is passed as the ACP
 * mcpServers parameter (tools arrive as hub__open_pane). By default Grok runs in
 * yolo mode: it works without prompts only under bypass, otherwise
 * `_meta.yoloMode = false` and permissions are requested; acceptEdits/plan are handled by
 * client-side policy by tool kind. Tested on grok 1.0.70.
 */

export const grokAdapter = createAcpAdapter({
  kind: 'grok',
  label: 'Grok CLI',
  mcpViaParam: true,
  command: () => ({ cmd: GROK_BIN, args: ['agent', 'stdio'] }),
  sessionMeta: (mode) => ({ yoloMode: mode === 'bypass' }),
});

export const grokDir = join(homedir(), '.grok');

export async function grokVersion(): Promise<string | null> {
  return (await runCli(GROK_BIN, ['--version'], 10_000))?.match(/(\d+\.\d+\.\d+)/)?.[1] ?? null;
}

export const grokLoggedIn = () => existsSync(join(grokDir, 'auth.json'));

/** Models from the CLI's own cache (~/.grok/models_cache.json), otherwise those declared in the last session. */
export function grokModels(): string[] {
  try {
    const d = JSON.parse(readFileSync(join(grokDir, 'models_cache.json'), 'utf8'));
    const list = Object.keys(d?.models ?? {});
    if (list.length) return list;
  } catch {}
  return kv.get<string[]>(modelsKey('grok'))?.value ?? [];
}
