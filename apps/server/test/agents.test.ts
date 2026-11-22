import './setup.ts';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { isAbsolute, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { AGENTS, DEFAULT_AGENT_KIND, findAgent } from '../src/agents/registry.ts';

const repo = fileURLToPath(new URL('../../../', import.meta.url));

function files(dir: string): string[] {
  return readdirSync(dir).flatMap((n) => {
    const p = join(dir, n);
    return statSync(p).isDirectory() ? files(p) : /\.tsx?$/.test(n) ? [p] : [];
  });
}

test('registered agents: unique kinds, a valid default, absolute sandbox paths', () => {
  const kinds = AGENTS.map((a) => a.kind);
  assert.equal(new Set(kinds).size, kinds.length);
  assert.ok(findAgent(DEFAULT_AGENT_KIND));
  for (const a of AGENTS) {
    assert.ok(a.label.trim(), a.kind);
    for (const p of a.statePaths()) assert.ok(isAbsolute(p), `${a.kind}: ${p}`);
  }
});

/**
 * The vendor-neutral core rule (AGENTS.md §1): agent kinds appear only in their modules
 * (apps/server/src/agents/), UI dictionaries (per-agent hints) and brand marks (AgentIcon).
 */
test('agent kinds are not hard-coded in the core or the UI', () => {
  const allowed = ['apps/server/src/agents/', 'apps/web/src/i18n/', 'apps/web/src/components/AgentIcon.tsx'];
  const roots = ['apps/server/src', 'apps/web/src', 'packages/shared/src'].map((r) => join(repo, r));
  const quoted = new RegExp(`['"\`](${AGENTS.map((a) => a.kind).join('|')})['"\`]`);
  const offenders = roots
    .flatMap(files)
    .map((f) => relative(repo, f))
    .filter((f) => !allowed.some((a) => f.startsWith(a)))
    .filter((f) => quoted.test(readFileSync(join(repo, f), 'utf8')));
  assert.deepEqual(offenders, []);
});
