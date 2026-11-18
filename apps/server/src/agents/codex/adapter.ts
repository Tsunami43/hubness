import { accessSync, constants, existsSync, readFileSync, realpathSync, statSync } from 'node:fs';
import { homedir } from 'node:os';
import { delimiter, dirname, join } from 'node:path';
import '../../config.ts';
import { kv } from '../../db/index.ts';
import { createAcpAdapter, modelsKey } from '../acp.ts';

/** Custom Codex ACP adapter command; defaults to the globally installed pinned codex-acp. */
const ACP_OVERRIDE = process.env.HUBNESS_CODEX_ACP?.trim().split(/\s+/) ?? null;

/**
 * Codex via the @zed-industries/codex-acp ACP adapter, plus the @openai/codex CLI for
 * login / logout. Both are installed globally with npm at pinned versions (Harness → Install);
 * nothing is downloaded on the fly. Updating installs the newer adapter version explicitly.
 * Custom adapter command: HUBNESS_CODEX_ACP. The hub MCP is passed as the ACP mcpServers
 * parameter. Codex modes (read-only / auto / full-access) are selected via session/set_mode
 * if the agent declares them; everything else is client-side policy by tool kind.
 */

export const CODEX_ACP_VERSION = '0.16.0';
export const CODEX_CLI_VERSION = '0.166.0';
export const CODEX_ACP_PKG = '@zed-industries/codex-acp';
export const CODEX_CLI_PKG = '@openai/codex';

/** First match of `bin` on the server's PATH. */
function findBin(bin: string): string | null {
  for (const dir of (process.env.PATH ?? '').split(delimiter)) {
    if (!dir) continue;
    const p = join(dir, bin);
    try {
      if (statSync(p).isFile()) return p;
    } catch {}
  }
  return null;
}

/** Version of the npm package a global bin links to (…/node_modules/<pkg>/bin/x.js). */
function binVersion(bin: string, pkg: string): string | null {
  const p = findBin(bin);
  if (!p) return null;
  try {
    let dir = dirname(realpathSync(p));
    for (let i = 0; i < 4; i++, dir = dirname(dir)) {
      const file = join(dir, 'package.json');
      if (!existsSync(file)) continue;
      const json = JSON.parse(readFileSync(file, 'utf8')) as { name?: string; version?: string };
      if (json.name === pkg) return json.version ?? null;
    }
  } catch {}
  return null;
}

/** Installed versions (null — not installed). A custom adapter command counts as installed. */
export function codexInstalled(): { acp: string | null; cli: string | null } {
  return {
    acp: ACP_OVERRIDE ? 'custom' : binVersion('codex-acp', CODEX_ACP_PKG),
    cli: binVersion('codex', CODEX_CLI_PKG),
  };
}

/** npm -g writes to <node prefix>/lib/node_modules; without write access it needs root. */
export function npmGlobalWritable(): boolean {
  try {
    accessSync(join(dirname(dirname(process.execPath)), 'lib', 'node_modules'), constants.W_OK);
    return true;
  } catch {
    return false;
  }
}

export const codexInstallCommand = (acp = CODEX_ACP_VERSION) => `npm i -g ${CODEX_ACP_PKG}@${acp} ${CODEX_CLI_PKG}@${CODEX_CLI_VERSION}`;

export function codexAcpCommand(): string[] {
  return ACP_OVERRIDE ?? ['codex-acp'];
}

export const codexAdapter = createAcpAdapter({
  kind: 'codex',
  label: 'Codex',
  mcpViaParam: true,
  command: () => {
    const [cmd, ...args] = codexAcpCommand();
    return { cmd, args };
  },
  modeIds: { ask: 'read-only', plan: 'read-only', acceptEdits: 'auto', bypass: 'full-access' },
});

export const codexDir = process.env.CODEX_HOME ?? join(homedir(), '.codex');

/** How Codex is authenticated: ChatGPT subscription (auth.json) or an API key in the environment. */
export function codexAuth(): string | null {
  if (existsSync(join(codexDir, 'auth.json'))) return 'ChatGPT / auth.json';
  if (process.env.CODEX_API_KEY) return 'CODEX_API_KEY';
  if (process.env.OPENAI_API_KEY) return 'OPENAI_API_KEY';
  return null;
}

export const codexModels = (): string[] => kv.get<string[]>(modelsKey('codex'))?.value ?? [];
