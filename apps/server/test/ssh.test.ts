import { tmp } from './setup.ts';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import type { SshBinding } from '@hubness/shared';
import { db, projects } from '../src/db/index.ts';
import { encrypt } from '../src/secrets.ts';
import { checkHost, checkName, createSsh, deleteSsh, disposeAllSsh, listSsh, normalizeKey, runtimeBase, sessionConfig, sessionContext, sessionSetup, setSshBindings, sshBindings, sweepSshRuntime, untestedSsh, updateSsh } from '../src/ssh.ts';

/** What a successful "Test" does: pin the host keys. Only then is a connection an agent's. */
const pin = (...ids: string[]) => ids.forEach((id) => db.prepare("UPDATE ssh_connections SET host_keys = 'host.example ssh-ed25519 AAAA' WHERE id = ?").run(id));

const status = (fn: () => unknown) => {
  try {
    fn();
  } catch (e) {
    return (e as { status?: number }).status ?? 0;
  }
  return 200;
};

const binding = (name: string, environment: string, hostKeys: string): SshBinding => ({
  connection: { id: name, name, host: '10.0.0.5', port: 2222, username: 'deploy', environment, description: 'Backend API', publicKey: '', hostKeys, createdAt: '', updatedAt: '' },
  note: 'deploy only after tests pass',
});

test('ssh: names are aliases an agent can type, nothing ssh would read as an option', () => {
  assert.equal(checkName(' prod-api '), 'prod-api');
  for (const bad of ['', 'bad name', '-x', 'a/b', 'a\nb', 'имя', 'x'.repeat(65), 'a*', 'a;b', 'a"b', "a'b", 'a$b']) assert.equal(status(() => checkName(bad)), 400, JSON.stringify(bad));
  assert.equal(checkName('x'.repeat(64)).length, 64);
  for (const ok of ['db.example.com', '10.0.0.5', '::1', 'fe80::1', 'my_host-1']) assert.equal(checkHost(` ${ok} `), ok);
  // brackets are URL syntax: OpenSSH can't resolve `[::1]`
  for (const bad of ['', 'a b', '-oProxyCommand=x', '[::1]', 'a\nHost *', 'h/../x', 'x'.repeat(254)]) assert.equal(status(() => checkHost(bad)), 400, JSON.stringify(bad));
});

test('ssh: the session config pins tested hosts and keeps the user’s own hosts (same text as ssh.rs)', () => {
  const list = [binding('prod-api', 'prod', '10.0.0.5 ssh-ed25519 AAAA'), binding('dev-api', 'dev', '')];
  const cfg = sessionConfig(list, '/d');
  assert.ok(
    cfg.includes(
      'Host prod-api\n  HostName 10.0.0.5\n  Port 2222\n  User deploy\n  IdentityAgent "/d/agent.sock"\n  IdentityFile "/d/keys/prod-api.pub"\n  IdentitiesOnly yes\n  UserKnownHostsFile "/d/known_hosts"\n  StrictHostKeyChecking yes\n',
    ),
  );
  assert.ok(cfg.includes('Host dev-api\n'));
  assert.ok(!cfg.includes('accept-new'), 'host keys are always checked: untested connections never get here');
  assert.ok(cfg.trimEnd().endsWith('Include ~/.ssh/config'), 'the include comes last: the hub’s aliases win');
  assert.equal(sessionConfig([], '/d').includes('Host '), false);
});

test('ssh: the agent context names each connection with its environment, description and note', () => {
  const ctx = sessionContext([binding('prod-api', 'prod', ''), { ...binding('box', '', ''), note: '' }]);
  assert.ok(ctx.startsWith('## SSH connections\n'));
  assert.ok(ctx.includes('- `prod-api` [prod] — deploy@10.0.0.5:2222 — Backend API In this project: deploy only after tests pass'));
  assert.ok(ctx.includes('- `box` — deploy@10.0.0.5:2222 — Backend API'), 'no brackets without an environment, no note part without a note');
  const multi = sessionContext([{ ...binding('m', 'dev', ''), connection: { ...binding('m', 'dev', '').connection, description: 'line one\nline two' }, note: 'a\nb' }]);
  assert.ok(multi.includes('line one line two In this project: a b'), 'one line per connection');
});

test('ssh: storage — the private key is encrypted at rest, edits validate, a new address unpins the host', () => {
  const made = createSsh({ name: 'store-a', host: 'example.com', username: 'root', environment: 'prod', description: ' API ' });
  assert.equal(made.port, 22);
  assert.equal(made.description, 'API');
  assert.match(made.publicKey, /^ssh-ed25519 \S+ hubness:store-a$/);
  const row = db.prepare('SELECT private_key FROM ssh_connections WHERE id = ?').get(made.id) as { private_key: string };
  assert.ok(!row.private_key.includes('PRIVATE KEY'), 'not stored in the clear');
  assert.deepEqual(readdirSync(runtimeBase()), [], 'the scratch folder of ssh-keygen is gone');
  assert.equal(existsSync(join(tmp, 'data', 'ssh')), false, 'keys never touch the data folder in the clear');

  assert.equal(status(() => createSsh({ name: 'STORE-A', host: 'h', username: 'u' })), 409);
  for (const bad of [{ host: 'a b' }, { host: '-oProxyCommand=x' }, { username: 'a@b' }, { username: '-l' }, { environment: 'x'.repeat(33) }, { description: 'x'.repeat(4001) }, { privateKey: 'not a key' }])
    assert.equal(status(() => createSsh({ name: 'store-bad', host: 'h', username: 'u', ...bad })), 400, JSON.stringify(bad));
  assert.equal(listSsh().some((c) => c.name === 'store-bad'), false, 'nothing is created by a refused request');

  db.prepare("UPDATE ssh_connections SET host_keys = 'example.com ssh-ed25519 AAAA' WHERE id = ?").run(made.id);
  assert.equal(updateSsh(made.id, { description: 'changed' }).hostKeys, 'example.com ssh-ed25519 AAAA', 'details keep the pinned keys');
  assert.equal(updateSsh(made.id, { port: 2200 }).hostKeys, '', 'another port is another server');
  assert.equal(status(() => updateSsh(made.id, { name: 'bad name' })), 400);
  assert.equal(status(() => updateSsh('missing', {})), 404);
  deleteSsh(made.id);
  assert.equal(status(() => deleteSsh(made.id)), 404);
});

test('ssh: bindings keep their order, belong to one owner and go with the project', () => {
  const a = createSsh({ name: 'bind-a', host: 'a.example', username: 'u' });
  const b = createSsh({ name: 'bind-b', host: 'b.example', username: 'u' });
  const p1 = projects.create('ssh-one', join(tmp, 'ssh-one'));
  const p2 = projects.create('ssh-two', join(tmp, 'ssh-two'));
  assert.deepEqual(setSshBindings({ project: p1.id }, { bindings: [{ connectionId: b.id, note: ' second first ' }, { connectionId: a.id }] }).map((x) => [x.connection.name, x.note]), [['bind-b', 'second first'], ['bind-a', '']]);
  assert.deepEqual(sshBindings({ project: p2.id }), []);
  setSshBindings({ project: p2.id }, { bindings: [{ connectionId: a.id }] });
  assert.equal(sshBindings({ project: p1.id }).length, 2, 'another project’s bindings are untouched');
  assert.equal(status(() => setSshBindings({ project: p1.id }, { bindings: [{ connectionId: a.id }, { connectionId: 'missing' }] })), 404);
  assert.equal(sshBindings({ project: p1.id }).length, 2, 'a refused list changes nothing');
  assert.equal(status(() => setSshBindings({ project: p1.id }, { bindings: [{ connectionId: a.id, note: 'x'.repeat(2001) }] })), 400);
  assert.deepEqual(setSshBindings({ project: p1.id }, { bindings: [] }), []);
  projects.delete(p2.id);
  assert.equal((db.prepare('SELECT COUNT(*) AS n FROM ssh_bindings WHERE project_id = ?').get(p2.id) as { n: number }).n, 0);
  deleteSsh(a.id);
  deleteSsh(b.id);
});

test('ssh: an agent turn gets an ssh-agent with the keys, aliases and wrappers; nothing is left after it', (t) => {
  try {
    execFileSync('ssh-agent', ['-h'], { stdio: 'ignore' });
  } catch (e) {
    if ((e as { code?: string }).code === 'ENOENT') return t.skip('OpenSSH is not installed');
  }
  const c = createSsh({ name: 'turn-prod', host: '203.0.113.10', port: 2222, username: 'deploy', environment: 'prod', description: 'API' });
  const p = projects.create('ssh-turn', join(tmp, 'ssh-turn'));
  assert.equal(sessionSetup([{ project: p.id }], 'sess-none'), null, 'no bindings: nothing is started');
  setSshBindings({ project: p.id }, { bindings: [{ connectionId: c.id, note: 'read only' }] });
  assert.equal(sessionSetup([{ project: p.id }], 'sess-untested'), null, 'not tested yet: not for the agent');
  assert.deepEqual(untestedSsh([{ project: p.id }, { project: p.id }]), ['turn-prod']);
  pin(c.id);
  assert.deepEqual(untestedSsh([{ project: p.id }]), []);

  const s = sessionSetup([{ project: p.id }, { project: p.id }], 'sess-0001-abcd-efgh')!;
  try {
    assert.ok(s.context.includes('`turn-prod` [prod] — deploy@203.0.113.10:2222 — API In this project: read only'));
    const sock = s.sock;
    assert.equal(s.env.SSH_AUTH_SOCK, undefined, 'the user’s own agent is not hidden');
    assert.deepEqual(s.failed, []);
    assert.ok(Buffer.byteLength(sock) < 100, `a unix socket path must be short: ${sock}`);
    assert.ok(s.env.PATH!.startsWith(join(s.mount, 'bin') + ':'));
    assert.equal(s.env.GIT_SSH_COMMAND, `'${join(s.mount, 'bin', 'ssh')}'`);
    assert.equal(statSync(s.mount).mode & 0o077, 0, 'the folder is private');

    const keys = execFileSync('ssh-add', ['-l'], { env: { ...process.env, SSH_AUTH_SOCK: sock }, encoding: 'utf8' });
    assert.match(keys, /hubness:turn-prod \(ED25519\)/, 'the agent holds the key');
    assert.equal(keys.trim().split('\n').length, 1, 'a connection bound twice is loaded once');
    for (const f of ['config', 'known_hosts', 'bin/ssh']) assert.ok(!readFileSync(join(s.mount, f), 'utf8').includes('PRIVATE KEY'), `${f} has no key`);

    const resolved = execFileSync('sh', ['-c', `${s.env.GIT_SSH_COMMAND} -G turn-prod`], { encoding: 'utf8' });
    assert.match(resolved, /^hostname 203\.0\.113\.10$/m);
    assert.match(resolved, /^user deploy$/m);
    assert.match(resolved, /^port 2222$/m);
    assert.match(resolved, new RegExp(`^identityagent ${sock.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}$`, 'm'));
    assert.match(resolved, /^stricthostkeychecking (yes|true)$/m, 'the pinned host key is insisted on');
    assert.equal(readFileSync(join(s.mount, 'known_hosts'), 'utf8'), 'host.example ssh-ed25519 AAAA\n');
    assert.match(resolved, /^identitiesonly yes$/m, 'each alias offers only its own key');
    assert.match(resolved, new RegExp(`^identityfile ${join(s.mount, 'keys', 'turn-prod.pub').replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}$`, 'm'));
    assert.equal(readFileSync(join(s.mount, 'keys', 'turn-prod.pub'), 'utf8').trim(), c.publicKey);
  } finally {
    s.dispose();
  }
  assert.equal(existsSync(s.mount), false, 'the folder is removed with the turn');
  assert.throws(() => execFileSync('ssh-add', ['-l'], { env: { ...process.env, SSH_AUTH_SOCK: s.sock }, stdio: 'pipe' }), 'the agent is gone');
  deleteSsh(c.id);
  assert.equal(sessionSetup([{ project: p.id }], 'sess-after'), null, 'a deleted connection leaves the project');
});

const agents = () => {
  try {
    return execFileSync('pgrep', ['-f', `ssh-agent -D -a ${runtimeBase()}`], { encoding: 'utf8' }).trim().split('\n').filter(Boolean);
  } catch {
    return [];
  }
};

test('ssh: a setup that fails leaves no agent and no folder; a bad key costs only its connection', (t) => {
  if (!existsSync('/proc/self/comm')) return t.skip('needs /proc');
  const good = createSsh({ name: 'fail-good', host: 'a.example', username: 'u' });
  const bad = createSsh({ name: 'fail-bad', host: 'b.example', username: 'u' });
  const p = projects.create('ssh-fail', join(tmp, 'ssh-fail'));
  setSshBindings({ project: p.id }, { bindings: [{ connectionId: bad.id }, { connectionId: good.id }] });
  pin(good.id, bad.id);
  const sealed = (id: string) => (db.prepare('SELECT private_key FROM ssh_connections WHERE id = ?').get(id) as { private_key: string }).private_key;
  const original = sealed(bad.id);

  // not decryptable (the secret key was rotated, the row was damaged): nothing is started
  db.prepare("UPDATE ssh_connections SET private_key = 'garbage' WHERE id = ?").run(bad.id);
  assert.throws(() => sessionSetup([{ project: p.id }], 'sess-fail-0001'));
  assert.deepEqual(agents(), [], 'no ssh-agent is left behind');
  assert.deepEqual(readdirSync(runtimeBase()), [], 'nor a folder');

  // decryptable but not a key (as a CRLF key stored by an older version): the other connection still works
  db.prepare('UPDATE ssh_connections SET private_key = ? WHERE id = ?').run(encrypt('ssh', bad.id, 'not a key\n'), bad.id);
  const s = sessionSetup([{ project: p.id }], 'sess-fail-0002')!;
  try {
    assert.deepEqual(s.failed, ['fail-bad']);
    assert.ok(s.context.includes('`fail-good`') && !s.context.includes('fail-bad'), 'the agent is told only about what works');
    const cfg = readFileSync(join(s.mount, 'config'), 'utf8');
    assert.ok(cfg.includes('Host fail-good\n') && !cfg.includes('Host fail-bad'));
  } finally {
    s.dispose();
  }
  assert.deepEqual(agents(), []);

  // nothing loads at all: no half-made setup
  db.prepare('UPDATE ssh_connections SET private_key = ? WHERE id = ?').run(encrypt('ssh', good.id, 'not a key either\n'), good.id);
  assert.throws(() => sessionSetup([{ project: p.id }], 'sess-fail-0003'), /ssh-add refused the keys of fail-bad, fail-good/);
  assert.deepEqual(agents(), []);
  assert.deepEqual(readdirSync(runtimeBase()), []);
  void original;
  deleteSsh(good.id);
  deleteSsh(bad.id);
});

test('ssh: an imported key with CRLF line ends is stored as OpenSSH reads it and works in a turn', (t) => {
  try {
    execFileSync('ssh-keygen', ['-q', '-t', 'ed25519', '-N', '', '-C', 'mine', '-f', join(tmp, 'crlf-key')]);
  } catch {
    return t.skip('OpenSSH is not installed');
  }
  const pem = readFileSync(join(tmp, 'crlf-key'), 'utf8');
  assert.equal(normalizeKey(`\r\n${pem.replace(/\n/g, '\r\n')}\r\n\r\n`), pem);
  assert.equal(normalizeKey(pem.trimEnd()), pem, 'one final newline');
  const c = createSsh({ name: 'crlf', host: 'h.example', username: 'u', privateKey: pem.replace(/\n/g, '\r\n') });
  assert.equal(c.publicKey.split(' ').slice(0, 2).join(' '), readFileSync(join(tmp, 'crlf-key.pub'), 'utf8').split(' ').slice(0, 2).join(' '));
  const p = projects.create('ssh-crlf', join(tmp, 'ssh-crlf'));
  setSshBindings({ project: p.id }, { bindings: [{ connectionId: c.id }] });
  pin(c.id);
  const s = sessionSetup([{ project: p.id }], 'sess-crlf')!;
  try {
    assert.deepEqual(s.failed, [], 'the agent took the key');
    assert.match(execFileSync('ssh-add', ['-l'], { env: { ...process.env, SSH_AUTH_SOCK: s.sock }, encoding: 'utf8' }), /ED25519/);
  } finally {
    s.dispose();
  }

  // a key with a passphrase can't be used unattended
  execFileSync('ssh-keygen', ['-q', '-t', 'ed25519', '-N', 'secret', '-f', join(tmp, 'locked-key')]);
  assert.equal(status(() => createSsh({ name: 'locked', host: 'h.example', username: 'u', privateKey: readFileSync(join(tmp, 'locked-key'), 'utf8') })), 400);
  deleteSsh(c.id);
});

test('ssh: many connections — each alias offers one key; hub exit and the next start leave nothing', (t) => {
  if (!existsSync('/proc/self/comm')) return t.skip('needs /proc');
  const p = projects.create('ssh-many', join(tmp, 'ssh-many'));
  const made = Array.from({ length: 8 }, (_, i) => createSsh({ name: `many-${i}`, host: `h${i}.example`, username: 'u' }));
  setSshBindings({ project: p.id }, { bindings: made.map((c) => ({ connectionId: c.id })) });
  pin(...made.map((c) => c.id));
  const s = sessionSetup([{ project: p.id }], 'sess-many')!;
  assert.equal(execFileSync('ssh-add', ['-l'], { env: { ...process.env, SSH_AUTH_SOCK: s.sock }, encoding: 'utf8' }).trim().split('\n').length, 8);
  const resolved = execFileSync('sh', ['-c', `${s.env.GIT_SSH_COMMAND} -G many-7`], { encoding: 'utf8' });
  assert.deepEqual(resolved.split('\n').filter((l) => l.startsWith('identityfile ')), [`identityfile ${join(s.mount, 'keys', 'many-7.pub')}`], 'not eight keys against MaxAuthTries');
  assert.equal(agents().length, 1);

  // the hub exits while the turn runs
  disposeAllSsh();
  assert.deepEqual(agents(), []);
  assert.equal(existsSync(s.mount), false, `left: ${existsSync(s.mount) ? readdirSync(s.mount, { recursive: true }) : ''}`);

  // a crash: the agent and its folder stay — the next start sweeps them
  const crashed = sessionSetup([{ project: p.id }], 'sess-crashed')!;
  assert.equal(agents().length, 1);
  sweepSshRuntime();
  assert.deepEqual(agents(), [], 'the leftover agent is stopped');
  assert.deepEqual(readdirSync(runtimeBase()), []);
  crashed.dispose();
  made.forEach((c) => deleteSsh(c.id));
});

test('ssh: of several bound connections the agent gets the tested ones; the rest are named for the person', (t) => {
  if (!existsSync('/proc/self/comm')) return t.skip('needs /proc');
  const ready = createSsh({ name: 'mix-ready', host: 'a.example', username: 'u' });
  const fresh = createSsh({ name: 'mix-fresh', host: 'b.example', username: 'u' });
  const p = projects.create('ssh-mix', join(tmp, 'ssh-mix'));
  setSshBindings({ project: p.id }, { bindings: [{ connectionId: fresh.id }, { connectionId: ready.id }] });
  pin(ready.id);
  assert.deepEqual(untestedSsh([{ project: p.id }]), ['mix-fresh']);
  const s = sessionSetup([{ project: p.id }], 'sess-mix')!;
  try {
    const cfg = readFileSync(join(s.mount, 'config'), 'utf8');
    assert.ok(cfg.includes('Host mix-ready\n') && !cfg.includes('mix-fresh'));
    assert.ok(s.context.includes('`mix-ready`') && !s.context.includes('mix-fresh'), 'the agent is not told about what it can’t use');
    assert.deepEqual(readdirSync(join(s.mount, 'keys')), ['mix-ready.pub']);
    assert.equal(execFileSync('ssh-add', ['-l'], { env: { ...process.env, SSH_AUTH_SOCK: s.sock }, encoding: 'utf8' }).trim().split('\n').length, 1, 'only its key is loaded');
  } finally {
    s.dispose();
  }
  // a new address unpins: back to waiting for a test
  updateSsh(ready.id, { host: 'c.example' });
  assert.deepEqual(untestedSsh([{ project: p.id }]), ['mix-fresh', 'mix-ready']);
  assert.equal(sessionSetup([{ project: p.id }], 'sess-mix-2'), null);
  deleteSsh(ready.id);
  deleteSsh(fresh.id);
});
