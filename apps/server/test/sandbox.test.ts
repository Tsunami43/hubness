import { makeRepo, tmp } from './setup.ts';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

// the agents' folders in the temporary HOME of setup.ts (the modules read it when they load)
const home = process.env.HOME!;
for (const d of ['.claude/projects', '.claude/skills', '.codex/sessions', '.config/opencode', '.local/share/opencode/storage']) mkdirSync(join(home, d), { recursive: true });
writeFileSync(join(home, '.claude/settings.json'), '{}');
writeFileSync(join(home, '.claude.json'), '{}');
writeFileSync(join(home, '.codex/state_5.sqlite'), '');

const { bwrapArgs, sandboxAvailable } = await import('../src/environments.ts');
const repo = makeRepo(join(tmp, 'work', 'repo'));
const h = (p: string) => join(home, p);

test("sandbox: agents' config is read-only, their runtime state writable, the deeper mount wins", () => {
  const args = bwrapArgs({ repos: [repo], cwd: repo });
  const at = (flag: string, p: string) => args.findIndex((a, i) => a === flag && args[i + 1] === p);
  for (const p of ['.claude', '.claude.json', '.codex/config.toml', '.codex/AGENTS.md', '.codex/rules', '.config/opencode'])
    assert.ok(at('--ro-bind', h(p)) >= 0, `${p} read-only`);
  for (const p of ['.claude/projects', '.claude/todos', '.codex', '.local/share/opencode']) assert.ok(at('--bind', h(p)) >= 0, `${p} writable`);
  assert.ok(at('--bind', h('.claude/projects')) > at('--ro-bind', h('.claude')), 'history folder mounted over the read-only config folder');
  assert.ok(at('--ro-bind', h('.codex/config.toml')) > at('--bind', h('.codex')), 'config mounted over the writable state folder');
  assert.equal(readFileSync(h('.codex/config.toml'), 'utf8'), '', 'a missing config.toml exists (empty) so the sandbox cannot create it');
  assert.ok(existsSync(h('.claude/todos')), 'runtime folders exist to be mounted');
});

test('sandbox (bwrap): writing agent settings fails, session history stays writable', (t) => {
  if (!sandboxAvailable()) return t.skip('bubblewrap is not installed');
  const args = bwrapArgs({ repos: [repo], cwd: repo });
  const write = (p: string) => spawnSync('bwrap', [...args, '--', 'sh', '-c', 'echo x >> "$1"', 'sh', h(p)], { encoding: 'utf8' }).status;
  for (const p of ['.claude/settings.json', '.claude/settings.local.json', '.claude/skills/evil.md', '.claude.json', '.codex/config.toml', '.config/opencode/opencode.json'])
    assert.notEqual(write(p), 0, `${p} must not be writable`);
  for (const p of ['.claude/projects/s.jsonl', '.claude/todos/t.json', '.codex/sessions/s.jsonl', '.codex/state_5.sqlite-wal', '.local/share/opencode/storage/s.json'])
    assert.equal(write(p), 0, `${p} must stay writable`);
  assert.equal(readFileSync(h('.claude/settings.json'), 'utf8'), '{}');
  assert.ok(!existsSync(h('.claude/settings.local.json')));
});
