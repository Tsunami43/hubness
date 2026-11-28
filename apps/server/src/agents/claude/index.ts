import { copyFileSync, existsSync, mkdirSync, readdirSync } from 'node:fs';
import { homedir } from 'node:os';
import { basename, join } from 'node:path';
import { config } from '../../config.ts';
import { notFound, parseVersion } from '../kit.ts';
import type { AgentModule } from '../types.ts';
import { CLAUDE_BIN, claudeProfile, profileConfigDir } from './accounts.ts';
import { claudeAdapter, claudeVersion } from './adapter.ts';
import { claudeExtensions, claudeMcp } from './extensions.ts';
import { claudeHarness, claudeSignedIn, KIND, setClaudeDefaultModel } from './harness.ts';
import { claudeModelList } from './models.ts';

/** Claude stores transcripts by folder path: ~/.claude/projects/<path with everything except [A-Za-z0-9] → '-'>. */
const projectDir = (cwd: string, account?: string) => join(profileConfigDir(claudeProfile(account)), 'projects', cwd.replace(/[^a-zA-Z0-9]/g, '-'));

/**
 * What Claude Code writes while it works (transcripts, todos, login refresh…) — writable in the
 * sandbox. The rest of its config folder (settings with hooks and the scripts they run, plugins,
 * skills) and ~/.claude.json (MCP servers) stay read-only: the CLI saves its own state there
 * when it can and goes on for the session when it can't.
 */
const RUNTIME_DIRS = ['projects', 'sessions', 'session-env', 'todos', 'tasks', 'plans', 'shell-snapshots', 'file-history', 'statsig', 'telemetry', 'debug', 'logs', 'ide', 'cache', 'paste-cache', 'image-cache', 'backups'];
const RUNTIME_FILES = ['.credentials.json', 'history.jsonl', 'stats-cache.json'];
// legacy CLAUDE_CONFIG_DIR profiles live in the hub data
const profilesDir = () => join(config.dataDir, 'claude-profiles');

function claudeStatePaths(): string[] {
  const profiles = existsSync(profilesDir()) ? readdirSync(profilesDir()).map((d) => join(profilesDir(), d)) : [];
  return [join(homedir(), '.claude'), ...profiles].flatMap((dir) => {
    if (!existsSync(dir)) return [];
    // a folder missing under a read-only one can't be created inside the sandbox
    for (const d of RUNTIME_DIRS) mkdirSync(join(dir, d), { recursive: true });
    return [...RUNTIME_DIRS, ...RUNTIME_FILES].map((e) => join(dir, e));
  });
}

export const claudeModule: AgentModule = {
  kind: KIND,
  label: 'Claude Code',
  adapter: claudeAdapter,
  modelStyle: 'options',
  async status() {
    const v = await claudeVersion();
    return { available: Boolean(v), detail: v ? `CLI ${v}` : notFound(CLAUDE_BIN), version: parseVersion(v), signedIn: v ? claudeSignedIn() : null };
  },
  async models() {
    // the list the CLI shows in /model; "default" = no --model, so it isn't offered as a choice
    const options = (await claudeModelList()).filter((m) => m.id !== 'default');
    return { models: options.map((m) => m.id), options };
  },
  harness: claudeHarness,
  setDefaultModel: setClaudeDefaultModel,
  statePaths: claudeStatePaths,
  configPaths: () => [join(homedir(), '.claude'), join(homedir(), '.claude.json'), profilesDir()],
  mcp: claudeMcp,
  extensions: claudeExtensions,
  // Claude Code reads CLAUDE.md, not AGENTS.md
  contextFiles: { 'CLAUDE.md': '@AGENTS.md\n' },
  relocate(s, from, to) {
    if (!s.agentSessionId) return false;
    const src = join(projectDir(from, s.agent.account), `${s.agentSessionId}.jsonl`);
    if (!existsSync(src)) return false;
    const dstDir = projectDir(to, s.agent.account);
    mkdirSync(dstDir, { recursive: true });
    copyFileSync(src, join(dstDir, basename(src)));
    return true;
  },
};
