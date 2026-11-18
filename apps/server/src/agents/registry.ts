import { HttpError } from '../errors.ts';
import type { AgentModule } from './types.ts';
import { claudeModule } from './claude/index.ts';
import { codexModule } from './codex/index.ts';
import { grokModule } from './grok/index.ts';
import { opencodeModule } from './opencode/index.ts';

/**
 * Registered agents, in UI order. The only place that lists agents: adding one is
 * a new module in agents/<kind>/ plus a line here.
 */
export const AGENTS: readonly AgentModule[] = [opencodeModule, claudeModule, grokModule, codexModule];

/** Agent preselected for a new session (and for "Ask" without a chosen agent). */
export const DEFAULT_AGENT_KIND = claudeModule.kind;

const byKind = new Map(AGENTS.map((a) => [a.kind, a]));

export const findAgent = (kind: string): AgentModule | undefined => byKind.get(kind);

export function agentOr404(kind: string): AgentModule {
  const a = byKind.get(kind);
  if (!a) throw new HttpError(404, 'Unknown agent');
  return a;
}
