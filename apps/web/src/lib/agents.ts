import type { AgentConfig, AgentInfo, AgentKind, AgentsResponse } from '@hubness/shared';
import { api } from './api';
import { qk, queryClient, useApi } from './query';

/**
 * Registered agents as the server lists them (/api/agents): order, names, model style,
 * the default one. The UI knows agents only from here — no agent kinds in the code.
 */

export function useAgents(): AgentsResponse | null {
  return useApi(qk.agents, () => api.agents(), { staleTime: 60_000 }).data ?? null;
}

/** Re-read the list (after install / login on the Harness page). */
export const reloadAgents = () => queryClient.invalidateQueries({ queryKey: qk.agents });

export const agentInfo = (kind: AgentKind): AgentInfo | undefined =>
  queryClient.getQueryData<AgentsResponse>(qk.agents)?.agents.find((a) => a.kind === kind);

/** Display name; the kind itself until the list has loaded. */
export const agentName = (kind: AgentKind): string => agentInfo(kind)?.label ?? kind;

/** A config for a new session: the stored one if its agent is still registered, otherwise the default agent. */
export function validAgent(stored: AgentConfig | null | undefined, agents: AgentsResponse | null): AgentConfig | null {
  if (!agents) return stored ?? null;
  if (stored && agents.agents.some((a) => a.kind === stored.kind)) return stored;
  return { kind: agents.defaultKind, permissionMode: stored?.permissionMode ?? 'ask' };
}
