import { tmp } from './setup.ts';
import { test } from 'node:test';
import { chmodSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { HubEvent, PermissionMode, Session } from '@hubness/shared';

/**
 * Normalizer snapshots: each fixture is recorded agent output (synthetic, shaped like real
 * logs/<session>.jsonl); a fake CLI replays it and the real adapter turns it into feed events.
 * After an agent CLI version bump, record a fresh fixture from a real run and update the
 * snapshots with `node --test --test-update-snapshots test/normalizers.test.ts`.
 */

const fixtures = fileURLToPath(new URL('./fixtures/', import.meta.url));
const replay = join(fixtures, 'replay-agent.ts');
const node = `${process.execPath} --disable-warning=ExperimentalWarning`;

// agent binaries are read when their modules load: point them at the replay agent first
const wrapper = (name: string, mode: string) => {
  const p = join(tmp, name);
  writeFileSync(p, `#!/bin/sh\nREPLAY_MODE=${mode} exec ${node} ${replay}\n`);
  chmodSync(p, 0o755);
  return p;
};
process.env.HUBNESS_CLAUDE_BIN = wrapper('fake-claude', 'stream');
process.env.HUBNESS_OPENCODE_BIN = wrapper('fake-opencode', 'acp');
process.env.HUBNESS_CODEX_ACP = wrapper('fake-codex', 'acp');

const { findAgent } = await import('../src/agents/registry.ts');
type RunContext = import('../src/agents/types.ts').RunContext;

/** Run one turn of `kind` over a fixture; the feed as the UI would end up with it. */
async function replayTurn(kind: string, fixture: string, permissionMode: PermissionMode = 'ask') {
  const feed = new Map<string, HubEvent>();
  const statuses: string[] = [];
  let agentSessionId: string | null = null;
  const session = { id: 's1', agent: { kind, permissionMode }, agentSessionId: null } as unknown as Session;
  const ctx: RunContext = {
    session,
    cwd: tmp,
    prompt: 'do the thing',
    attachments: [],
    extraDirs: [],
    systemPrompt: null,
    mcpServers: [],
    env: { REPLAY_FIXTURE: join(fixtures, 'agents', fixture) },
    wrap: null,
    put: (uid, body) => void feed.set(uid, { ...body, uid, seq: 0, ts: '' } as HubEvent),
    patch: (uid, patch) => {
      const prev = feed.get(uid);
      if (prev) feed.set(uid, { ...prev, ...patch } as HubEvent);
    },
    get: (uid) => feed.get(uid),
    setStatus: (s) => void statuses.push(s),
    setAgentSessionId: (id) => void (agentSessionId = id),
    history: () => [],
    raw: () => {},
  };
  const handle = findAgent(kind)!.adapter.start(ctx);
  await handle.done;
  // stable across runs: uids carry run tags and timestamps, durations vary
  const counters = new Map<string, number>();
  const events = [...feed.values()].map((e) => {
    const prefix = e.uid.split(':')[0]!;
    const n = (counters.get(prefix) ?? 0) + 1;
    counters.set(prefix, n);
    const { seq: _s, ts: _t, durationMs: _d, ...rest } = e as HubEvent & { durationMs?: number };
    const toolId = 'toolId' in rest && typeof rest.toolId === 'string' ? rest.toolId.replace(/^plan:.*/, 'plan:<run>') : undefined;
    return { ...rest, uid: `${prefix}#${n}`, ...(toolId ? { toolId } : {}), ...('requestId' in rest ? { requestId: '<id>' } : {}) };
  });
  return { agentSessionId, statuses, events };
}

test('claude-code: streaming text, thinking, tools ok / error, limits and result', async (t) => {
  t.assert.snapshot(await replayTurn('claude-code', 'claude-tool-turn.jsonl'));
});

test('claude-code: a failed turn shows the CLI error', async (t) => {
  t.assert.snapshot(await replayTurn('claude-code', 'claude-error.jsonl'));
});

test('acp (codex): chunks, permission auto-allowed in bypass, tool output, diff edit, plan', async (t) => {
  t.assert.snapshot(await replayTurn('codex', 'acp-tool-turn.jsonl', 'bypass'));
});

test('acp (opencode): provider retry notice and the final error with its real cause', async (t) => {
  t.assert.snapshot(await replayTurn('opencode', 'acp-provider-error.jsonl'));
});
