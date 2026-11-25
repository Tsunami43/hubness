import { existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import type { PluginInfo, SubagentInfo } from '@hubness/shared';
import { readJson } from '../kit.ts';
import { jsonStore, opencodeFormat } from '../mcp-config.ts';
import type { ExtensionDir, ExtensionLayout } from '../types.ts';

/**
 * OpenCode's own config in $XDG_CONFIG_HOME/opencode (~/.config/opencode): MCP servers in
 * `mcp` of opencode.json, global AGENTS.md, and agent / command / skill folders (singular
 * and plural names both work). A project adds its opencode.json and .opencode/.
 */

const configDir = () => join(process.env.XDG_CONFIG_HOME || join(homedir(), '.config'), 'opencode');

/** opencode.json, or opencode.jsonc when only that one exists. */
function userFile(): string {
  const json = join(configDir(), 'opencode.json');
  const jsonc = join(configDir(), 'opencode.jsonc');
  return !existsSync(json) && existsSync(jsonc) ? jsonc : json;
}

export const opencodeMcp = jsonStore({
  ...opencodeFormat,
  userFile,
  projectFiles: (dir) => [{ file: join(dir, 'opencode.json') }, { file: join(dir, 'opencode.jsonc') }],
});

/** Agents and plugins listed in the config itself. */
function fromConfig(): { agents: SubagentInfo[]; plugins: PluginInfo[] } {
  const path = userFile();
  const cfg = readJson(path);
  const agents: SubagentInfo[] = [];
  if (cfg?.agent && typeof cfg.agent === 'object' && !Array.isArray(cfg.agent))
    for (const [name, a] of Object.entries(cfg.agent as Record<string, any>)) {
      if (!a || typeof a !== 'object') continue;
      agents.push({
        name,
        description: typeof a.description === 'string' ? Array.from(a.description).slice(0, 1000).join('') : '',
        model: typeof a.model === 'string' ? a.model : null,
        path,
        scope: 'user',
      });
    }
  const plugins: PluginInfo[] = (Array.isArray(cfg?.plugin) ? cfg.plugin : [])
    .filter((p: unknown): p is string => typeof p === 'string')
    .map((name: string) => ({ name, version: null, enabled: true, path, scope: 'user' as const }));
  return { agents, plugins };
}

export function opencodeExtensions(project: string | null): ExtensionLayout {
  const base = configDir();
  const both = (name: string): ExtensionDir[] => [
    { dir: join(base, name), scope: 'user' },
    { dir: join(base, `${name}s`), scope: 'user' },
    ...(project
      ? [
          { dir: join(project, '.opencode', name), scope: 'project' as const },
          { dir: join(project, '.opencode', `${name}s`), scope: 'project' as const },
        ]
      : []),
  ];
  return {
    skills: both('skill'),
    rules: [
      { label: 'AGENTS.md', path: join(base, 'AGENTS.md'), scope: 'user' },
      ...(project ? [{ label: 'AGENTS.md', path: join(project, 'AGENTS.md'), scope: 'project' as const }] : []),
    ],
    agents: both('agent'),
    commands: both('command'),
    more: fromConfig,
  };
}
