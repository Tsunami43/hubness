import './setup.ts';
import './dead-agent-env.ts';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { events, sessions } from '../src/db/index.ts';
import { sendMessage } from '../src/runtime.ts';

test('an agent CLI that dies at start fails the turn instead of crashing the hub (EPIPE on stdin)', async () => {
  const s = sessions.create({ projectId: null, title: 'dead agent', agent: { kind: 'claude-code' }, cwd: process.env.HUBNESS_HOME! });
  sendMessage(s.id, 'hello');
  for (let i = 0; i < 50 && ['running', 'waiting'].includes(sessions.get(s.id)!.status); i++) await new Promise((r) => setTimeout(r, 100));
  assert.equal(sessions.get(s.id)!.status, 'error', 'the turn ends with an error');
  assert.ok(events.list(s.id).some((e) => e.kind === 'notice'), 'the failure is shown in the feed');
  // still alive: a second message works the same way
  sendMessage(s.id, 'again');
  for (let i = 0; i < 50 && ['running', 'waiting'].includes(sessions.get(s.id)!.status); i++) await new Promise((r) => setTimeout(r, 100));
  assert.equal(events.list(s.id).filter((e) => e.kind === 'user').length, 2);
  assert.equal(sessions.get(s.id)!.status, 'error');
});
