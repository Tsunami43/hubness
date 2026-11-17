import { existsSync, mkdirSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');

const envFile = join(repoRoot, '.env');
if (existsSync(envFile)) process.loadEnvFile(envFile);

const dataDir = resolve(process.env.HUBNESS_DATA_DIR ?? join(repoRoot, '.data'));
mkdirSync(dataDir, { recursive: true });

/** Hub working folder: inbox for new chats, projects for projects, home — the system project. */
const hubHome = resolve(process.env.HUBNESS_HOME ?? join(homedir(), 'hub'));

export const config = {
  port: Number(process.env.HUBNESS_PORT ?? 8787),
  host: process.env.HUBNESS_HOST ?? '127.0.0.1',
  dataDir,
  dbPath: join(dataDir, 'hubness.sqlite'),
  logsDir: join(dataDir, 'logs'),
  chatsDir: join(dataDir, 'chats'),
  hubHome,
  inboxDir: join(hubHome, 'inbox'),
  projectsDir: join(hubHome, 'projects'),
  homeProjectDir: join(hubHome, 'home'),
  /** Hub API URL for the agent's MCP server. */
  get selfUrl() {
    return `http://127.0.0.1:${this.port}`;
  },
  webDist: join(repoRoot, 'apps/web/dist'),
  production: process.env.NODE_ENV === 'production',
};

mkdirSync(config.logsDir, { recursive: true });
mkdirSync(config.chatsDir, { recursive: true });
for (const d of [config.inboxDir, config.projectsDir, config.homeProjectDir]) mkdirSync(d, { recursive: true });

/**
 * Environment for child processes (agents, Run, Shell, CLI): without hub settings and
 * secrets (HUBNESS_*, including the secrets encryption key) and without the internal
 * pnpm/npm variables the hub itself was started with.
 */
export function childEnv(): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(process.env)) {
    if (v === undefined || k.startsWith('HUBNESS_')) continue;
    if (/^(npm_|PNPM_|pnpm_)/.test(k) || k === 'INIT_CWD') continue;
    out[k] = v;
  }
  return out;
}
