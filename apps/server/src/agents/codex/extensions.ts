import { homedir } from 'node:os';
import { join } from 'node:path';
import { tomlStore } from '../mcp-config.ts';
import type { ExtensionLayout } from '../types.ts';

/**
 * Codex's own config in CODEX_HOME (~/.codex): MCP servers as `[mcp_servers.<name>]` in
 * config.toml (stdio or streamable HTTP, headers in `http_headers`), global AGENTS.md,
 * skills and custom prompts. A project may add .codex/config.toml.
 */

const home = () => process.env.CODEX_HOME || join(homedir(), '.codex');

export const codexMcp = tomlStore({
  headersKey: 'http_headers',
  transports: ['stdio', 'http'],
  userFile: () => join(home(), 'config.toml'),
  projectFiles: (dir) => [join(dir, '.codex', 'config.toml')],
});

export function codexExtensions(project: string | null): ExtensionLayout {
  const base = home();
  return {
    skills: [
      { dir: join(base, 'skills'), scope: 'user' },
      ...(project
        ? [
            { dir: join(project, '.codex', 'skills'), scope: 'project' as const },
            { dir: join(project, '.agents', 'skills'), scope: 'project' as const },
          ]
        : []),
    ],
    rules: [
      { label: 'AGENTS.md', path: join(base, 'AGENTS.md'), scope: 'user' },
      { label: 'AGENTS.override.md', path: join(base, 'AGENTS.override.md'), scope: 'user', ifExists: true },
      ...(project ? [{ label: 'AGENTS.md', path: join(project, 'AGENTS.md'), scope: 'project' as const }] : []),
    ],
    agents: [],
    commands: [{ dir: join(base, 'prompts'), scope: 'user' }],
  };
}
