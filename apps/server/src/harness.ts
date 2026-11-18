import type { AgentInfo, AgentsResponse, HarnessInfo, HarnessResponse } from '@hubness/shared';
import { AGENTS, agentOr404, DEFAULT_AGENT_KIND } from './agents/registry.ts';
import { clearLatest } from './agents/kit.ts';

/**
 * Harness data of the registered agents: the light availability list for pickers
 * and the full per-agent info for the Harness page. Agent-specific reading lives in
 * each agent module; this file only caches.
 */

// each harness is read on its own (Harness page tab), cached for a minute
const cache = new Map<string, { at: number; data: Promise<{ info: HarnessInfo; checkedAt: string }> }>();
let agentsCache: { at: number; data: AgentInfo[] } | null = null;

/** After install, login, logout, update, settings — data is recomputed. */
export function invalidateHarness(kind?: string) {
  if (kind) {
    cache.delete(kind);
    clearLatest(kind);
  } else cache.clear();
  agentsCache = null;
}

/** One harness: versions, update, account, models… «refresh» also re-checks the latest version online. */
export function harnessOne(kind: string, refresh = false): Promise<{ info: HarnessInfo; checkedAt: string }> {
  const agent = agentOr404(kind);
  const hit = cache.get(kind);
  if (!refresh && hit && Date.now() - hit.at < 60_000) return hit.data;
  if (refresh) {
    clearLatest(kind);
    agentsCache = null;
  }
  const data = agent.harness(refresh).then(
    (info) => ({ info, checkedAt: new Date().toISOString() }),
    (e) => {
      cache.delete(kind);
      throw e;
    },
  );
  cache.set(kind, { at: Date.now(), data });
  return data;
}

/** All harnesses at once (kept for API clients; the UI reads them per tab). */
export async function harnessAll(refresh = false): Promise<HarnessResponse> {
  const all = await Promise.all(AGENTS.map((a) => harnessOne(a.kind, refresh)));
  return { harness: all.map((h) => h.info), checkedAt: new Date().toISOString() };
}

/** Agent availability for selection in the UI (faster than the full harness). */
export async function agentsList(): Promise<AgentsResponse> {
  if (!agentsCache || Date.now() - agentsCache.at >= 60_000) {
    const data = await Promise.all(
      AGENTS.map(async (a): Promise<AgentInfo> => ({ kind: a.kind, label: a.label, modelStyle: a.modelStyle, ...(await a.status()) })),
    );
    agentsCache = { at: Date.now(), data };
  }
  return { agents: agentsCache.data, defaultKind: DEFAULT_AGENT_KIND };
}
