import { homedir } from 'node:os';
import { join } from 'node:path';
import type { PluginInfo } from '@hubness/shared';
import { readJson, type Json } from '../kit.ts';
import { claudeFormat, jsonStore } from '../mcp-config.ts';
import type { ExtensionDir, ExtensionLayout } from '../types.ts';

/**
 * Claude Code's own config, as the CLI finds it: ~/.claude (or CLAUDE_CONFIG_DIR) and
 * ~/.claude.json (or $CLAUDE_CONFIG_DIR/.claude.json). MCP servers: user scope in
 * `mcpServers` of .claude.json; a project adds .mcp.json and its local scope
 * (`projects[<path>].mcpServers` of .claude.json).
 */

const configDir = () => process.env.CLAUDE_CONFIG_DIR || join(homedir(), '.claude');
const claudeJson = () => (process.env.CLAUDE_CONFIG_DIR ? join(process.env.CLAUDE_CONFIG_DIR, '.claude.json') : join(homedir(), '.claude.json'));

export const claudeMcp = jsonStore({
  ...claudeFormat,
  userFile: claudeJson,
  projectFiles: (dir) => [{ file: join(dir, '.mcp.json') }, { file: claudeJson(), pick: (cfg) => cfg.projects?.[dir]?.mcpServers }],
});

/** Installed plugins (plugins/installed_plugins.json, v1 or v2) and the skills they bring. */
function plugins(base: string): { plugins: PluginInfo[]; skills: ExtensionDir[] } {
  const installed = readJson(join(base, 'plugins', 'installed_plugins.json'))?.plugins;
  const enabled = readJson(join(base, 'settings.json'))?.enabledPlugins ?? {};
  const out: PluginInfo[] = [];
  const skills: ExtensionDir[] = [];
  if (installed && typeof installed === 'object' && !Array.isArray(installed)) {
    for (const [id, v] of Object.entries(installed as Json)) {
      const rec: Json | undefined = Array.isArray(v) ? v[0] : v;
      if (!rec || typeof rec !== 'object' || typeof rec.installPath !== 'string') continue;
      out.push({ name: id, version: typeof rec.version === 'string' ? rec.version : null, enabled: enabled[id] === true, path: rec.installPath, scope: 'plugin' });
      skills.push({ dir: join(rec.installPath, 'skills'), scope: 'plugin' });
    }
  }
  return { plugins: out, skills };
}

export function claudeExtensions(project: string | null): ExtensionLayout {
  const base = configDir();
  const both = (user: string, proj: string): ExtensionDir[] => [
    { dir: join(base, user), scope: 'user' },
    ...(project ? [{ dir: join(project, '.claude', proj), scope: 'project' as const }] : []),
  ];
  return {
    skills: both('skills', 'skills'),
    rules: [
      { label: 'CLAUDE.md', path: join(base, 'CLAUDE.md'), scope: 'user' },
      ...(project
        ? [
            { label: 'CLAUDE.md', path: join(project, 'CLAUDE.md'), scope: 'project' as const },
            { label: 'CLAUDE.local.md', path: join(project, 'CLAUDE.local.md'), scope: 'project' as const },
          ]
        : []),
    ],
    agents: both('agents', 'agents'),
    commands: both('commands', 'commands'),
    more: () => plugins(base),
  };
}
