import { homedir } from 'node:os';
import { join } from 'node:path';
import '../../config.ts';
import { kv } from '../../db/index.ts';

/** Claude Code CLI binary. */
export const CLAUDE_BIN = process.env.HUBNESS_CLAUDE_BIN ?? 'claude';

/**
 * Claude Code config folders. The hub uses one account — the usual ~/.claude and
 * ~/.claude.json. Multiple accounts (extra CLAUDE_CONFIG_DIR profiles) were removed from
 * the UI; profiles created back then stay in kv only so that old sessions resume in the
 * config folder where their transcript lives. New sessions always use the default one.
 */

export interface ClaudeProfile {
  id: string;
  label: string;
  /** null: the default profile (~/.claude). */
  dir: string | null;
}

const PROFILES_KEY = 'claude.profiles';
const DEFAULT: ClaudeProfile = { id: 'default', label: 'Default', dir: null };

/** Profile of an existing session (legacy profiles included); unknown → the default one. */
export function claudeProfile(id: string | undefined): ClaudeProfile {
  if (!id || id === 'default') return DEFAULT;
  return (kv.get<ClaudeProfile[]>(PROFILES_KEY)?.value ?? []).find((p) => p.id === id) ?? DEFAULT;
}

export const DEFAULT_CLAUDE_PROFILE = DEFAULT;
export const profileEnv = (p: ClaudeProfile): Record<string, string> => (p.dir ? { CLAUDE_CONFIG_DIR: p.dir } : {});
export const profileConfigDir = (p: ClaudeProfile) => p.dir ?? join(homedir(), '.claude');
export const profileClaudeJson = (p: ClaudeProfile) => (p.dir ? join(p.dir, '.claude.json') : join(homedir(), '.claude.json'));
