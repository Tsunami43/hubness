import { tmp } from './setup.ts';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { chmodSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { HubEvent, Session } from '@hubness/shared';

// A fake ACP agent that answers the handshake and then dies inside session/prompt:
// EXIT_ON=prompt exits right away, EXIT_ON=cancel exits when the hub sends session/cancel.
const agent = join(tmp, 'dying-acp.mjs');
writeFileSync(
  agent,
  `import { createInterface } from 'node:readline';
const out = (m) => process.stdout.write(JSON.stringify(m) + '\\n');
createInterface({ input: process.stdin }).on('line', (l) => {
  const m = JSON.parse(l);
  if (m.method === 'initialize') out({ jsonrpc: '2.0', id: m.id, result: { protocolVersion: 1, agentCapabilities: {} } });
  else if (m.method === 'session/new') out({ jsonrpc: '2.0', id: m.id, result: { sessionId: 'sess-1' } });
  else if (m.method === 'session/prompt' && process.env.EXIT_ON === 'prompt') process.exit(3);
  else if (m.method === 'session/cancel') process.exit(0);
});
`,
);
const bin = join(tmp, 'fake-codex');
writeFileSync(bin, `#!/bin/sh\nexec ${process.execPath} ${agent}\n`);
chmodSync(bin, 0o755);
process.env.HUBNESS_CODEX_ACP = bin;

const { findAgent } = await import('../src/agents/registry.ts');
type RunContext = import('../src/agents/types.ts').RunContext;

function start(exitOn: 'prompt' | 'cancel') {
  const feed = new Map<string, HubEvent>();
  let sid: string | null = null;
  const session = { id: 's1', agent: { kind: 'codex', permissionMode: 'ask' }, agentSessionId: null } as unknown as Session;
  const ctx: RunContext = {
    session,
    cwd: tmp,
    prompt: 'do the thing',
    attachments: [],
    extraDirs: [],
    systemPrompt: null,
    mcpServers: [],
    env: { EXIT_ON: exitOn },
    wrap: null,
    put: (uid, body) => void feed.set(uid, { ...body, uid, seq: 0, ts: '' } as HubEvent),
    patch: () => {},
    get: (uid) => feed.get(uid),
    setStatus: () => {},
    setAgentSessionId: (id) => void (sid = id),
    history: () => [],
    raw: () => {},
  };
  return { handle: findAgent('codex')!.adapter.start(ctx), feed, sessionId: () => sid };
}

const notices = (feed: Map<string, HubEvent>) =>
  [...feed.values()].flatMap((e) => (e.kind === 'notice' ? [`${e.level}: ${e.text.split('\n')[0]}`] : []));

test('acp: an agent that exits during session/prompt ends the turn with an error', async () => {
  const { handle, feed } = start('prompt');
  await handle.done;
  assert.deepEqual(notices(feed), ['error: Codex exited with code 3']);
});

test('acp: Stop while the agent exits instead of answering session/prompt still says Stopped', async () => {
  const { handle, feed, sessionId } = start('cancel');
  // the session exists: session/prompt goes out right after it
  while (!sessionId()) await new Promise((r) => setTimeout(r, 10));
  await new Promise((r) => setTimeout(r, 100));
  handle.stop('user');
  await handle.done;
  assert.deepEqual(notices(feed), ['info: Stopped']);
});
