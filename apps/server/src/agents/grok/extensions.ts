import { homedir } from 'node:os';
import { join } from 'node:path';
import { tomlStore } from '../mcp-config.ts';
import type { ExtensionDir, ExtensionLayout } from '../types.ts';

/**
 * Grok CLI's own config in GROK_HOME (~/.grok), per its user guide: MCP servers as
 * `[mcp_servers.<name>]` in config.toml (command/args/env, or url/headers with
 * `type = "sse"` for SSE), global AGENTS.md, skills, agents and commands. A project may
 * add .grok/config.toml and the same folders under .grok/.
 */

const home = () => process.env.GROK_HOME || join(homedir(), '.grok');

export const grokMcp = tomlStore({
  headersKey: 'headers',
  transports: ['stdio', 'http', 'sse'],
  userFile: () => join(home(), 'config.toml'),
  projectFiles: (dir) => [join(dir, '.grok', 'config.toml')],
});

export function grokExtensions(project: string | null): ExtensionLayout {
  const base = home();
  const both = (name: string): ExtensionDir[] => [
    { dir: join(base, name), scope: 'user' },
    ...(project ? [{ dir: join(project, '.grok', name), scope: 'project' as const }] : []),
  ];
  return {
    skills: both('skills'),
    rules: [
      { label: 'AGENTS.md', path: join(base, 'AGENTS.md'), scope: 'user' },
      ...(project ? [{ label: 'AGENTS.md', path: join(project, 'AGENTS.md'), scope: 'project' as const }] : []),
    ],
    agents: both('agents'),
    commands: both('commands'),
  };
}
