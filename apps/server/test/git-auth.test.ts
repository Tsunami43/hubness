import { makeRepo, tmp } from './setup.ts';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { readFileSync, writeFileSync } from 'node:fs';
import type { AddressInfo } from 'node:net';
import { join } from 'node:path';
import * as git from '../src/git.ts';
import { authProblem, deleteGitCredential, gitAuthFor, listGitCredentials, normalizeHost, setGitCredential } from '../src/git-auth.ts';
import { gitCredentialRows } from '../src/db/index.ts';

const sh = (cwd: string, ...args: string[]) => execFileSync('git', ['-C', cwd, ...args], { encoding: 'utf8' }).trim();

test('git auth: host normalisation and validation', () => {
  assert.equal(normalizeHost('https://user@GitLab.Example.com:8443/group/repo.git'), 'gitlab.example.com');
  assert.equal(normalizeHost(' github.com '), 'github.com');
  assert.equal(normalizeHost('bad host'), null);
  assert.equal(normalizeHost('-x.com'), null);
  assert.throws(() => setGitCredential('github.com', { username: 'u', kind: 'oauth', secret: 's' }), /kind/);
  assert.throws(() => setGitCredential('github.com', { username: 'u', kind: 'token', secret: 'a\nb' }), /Token/);
  assert.throws(() => setGitCredential('github.com', { username: '', kind: 'token', secret: 's' }), /Username/);
});

test('git auth: stored encrypted, listed without the secret, only for matching https hosts', () => {
  const list = setGitCredential('https://gitlab.example.com', { username: 'alice', kind: 'password', secret: 'pa$$word' });
  assert.deepEqual(
    list.map((c) => ({ host: c.host, username: c.username, kind: c.kind })),
    [{ host: 'gitlab.example.com', username: 'alice', kind: 'password' }],
  );
  assert.ok(!JSON.stringify(list).includes('pa$$word'));
  assert.ok(!gitCredentialRows.list()[0]!.secret.includes('pa$$word'), 'secret is encrypted in the DB');

  assert.deepEqual(gitAuthFor(['git@gitlab.example.com:group/repo.git']), { args: [], env: {} }, 'ssh remotes are not touched');
  assert.deepEqual(gitAuthFor(['https://github.com/a/b.git']), { args: [], env: {} }, 'no login for another host');
  const auth = gitAuthFor(['https://gitlab.example.com/group/repo.git']);
  assert.equal(auth.args[1], 'credential.helper=');
  assert.deepEqual(JSON.parse(auth.env.HUB_GIT_CREDENTIALS!), { 'gitlab.example.com': { username: 'alice', password: 'pa$$word' } });

  // git itself asks our helper
  const out = execFileSync('git', [...auth.args, 'credential', 'fill'], {
    input: 'protocol=https\nhost=gitlab.example.com\n\n',
    env: { ...process.env, ...auth.env, GIT_TERMINAL_PROMPT: '0' },
    encoding: 'utf8',
  });
  assert.match(out, /username=alice\npassword=pa\$\$word/);

  assert.deepEqual(deleteGitCredential('gitlab.example.com'), []);
  assert.deepEqual(listGitCredentials(), []);
});

test('git auth: access refusals are recognised', () => {
  assert.deepEqual(authProblem("fatal: could not read Username for 'https://gitlab.example.com': terminal prompts disabled", []), {
    host: 'gitlab.example.com',
    protocol: 'https',
    reason: 'missing',
  });
  assert.equal(authProblem("remote: HTTP Basic: Access denied\nfatal: Authentication failed for 'https://gitlab.example.com/g/r.git/'", [])?.reason, 'rejected');
  assert.equal(authProblem("fatal: unable to access 'https://x.org/r.git/': The requested URL returned error: 403", [])?.host, 'x.org');
  assert.deepEqual(authProblem('git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.', []), {
    host: 'github.com',
    protocol: 'ssh',
    reason: 'missing',
  });
  assert.deepEqual(authProblem('Host key verification failed.\nfatal: Could not read from remote repository.', ['gitlab.example.com']), {
    host: 'gitlab.example.com',
    protocol: 'ssh',
    reason: 'hostkey',
  });
  assert.equal(authProblem('fatal: refusing to merge unrelated histories', []), null);
});

/** Smart-HTTP git server (git http-backend) behind Basic auth. */
async function gitHttpServer(root: string, user: string, pass: string) {
  const expected = 'Basic ' + Buffer.from(`${user}:${pass}`).toString('base64');
  const server = createServer((req, res) => {
    if (req.headers.authorization !== expected) {
      res.writeHead(401, { 'WWW-Authenticate': 'Basic realm="test"' }).end();
      return;
    }
    const url = new URL(req.url ?? '/', 'http://x');
    const cgi = spawn('git', ['http-backend'], {
      env: {
        ...process.env,
        GIT_PROJECT_ROOT: root,
        GIT_HTTP_EXPORT_ALL: '1',
        REMOTE_USER: user,
        PATH_INFO: url.pathname,
        QUERY_STRING: url.search.slice(1),
        REQUEST_METHOD: req.method ?? 'GET',
        CONTENT_TYPE: req.headers['content-type'] ?? '',
      },
    });
    req.pipe(cgi.stdin);
    let buf = Buffer.alloc(0);
    let headersDone = false;
    cgi.stdout.on('data', (chunk: Buffer) => {
      if (headersDone) return void res.write(chunk);
      buf = Buffer.concat([buf, chunk]);
      const end = buf.indexOf('\r\n\r\n');
      if (end < 0) return;
      headersDone = true;
      let status = 200;
      for (const line of buf.subarray(0, end).toString().split('\r\n')) {
        const i = line.indexOf(':');
        const k = line.slice(0, i).trim();
        const v = line.slice(i + 1).trim();
        if (k.toLowerCase() === 'status') status = Number(v.split(' ')[0]);
        else res.setHeader(k, v);
      }
      res.writeHead(status);
      res.write(buf.subarray(end + 4));
    });
    cgi.stdout.on('end', () => res.end());
  });
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
  return { server, port: (server.address() as AddressInfo).port };
}

test('git auth: fetch / push over https use the stored login; refusals come back as auth problems', async () => {
  const root = join(tmp, 'git-http');
  execFileSync('git', ['init', '-q', '--bare', '-b', 'main', join(root, 'r.git')]);
  const { server, port } = await gitHttpServer(root, 'alice', 's3cret');
  try {
    const dir = makeRepo(join(tmp, 'git-auth-work'), `http://127.0.0.1:${port}/r.git`);
    sh(dir, 'config', 'user.email', 't@example.com');
    sh(dir, 'config', 'user.name', 'T');
    writeFileSync(join(dir, 'a.txt'), 'one\n');
    sh(dir, 'add', '-A');
    sh(dir, 'commit', '-qm', 'first');

    const none = await git.ops.push(dir);
    assert.equal(none.ok, false);
    assert.deepEqual(none.auth, { host: '127.0.0.1', protocol: 'https', reason: 'missing' });

    setGitCredential('127.0.0.1', { username: 'alice', kind: 'password', secret: 'wrong' });
    const wrong = await git.ops.push(dir);
    assert.equal(wrong.ok, false);
    assert.equal(wrong.auth?.reason, 'rejected');

    setGitCredential('127.0.0.1', { username: 'alice', kind: 'token', secret: 's3cret' });
    const pushed = await git.ops.push(dir);
    assert.equal(pushed.ok, true, pushed.output);
    assert.equal(sh(dir, 'rev-parse', '@{u}'), sh(dir, 'rev-parse', 'HEAD'));
    assert.equal((await git.ops.fetch(dir)).ok, true);
    assert.ok(!readFileSync(join(dir, '.git/config'), 'utf8').includes('credential'), 'nothing written to the repo config');
  } finally {
    deleteGitCredential('127.0.0.1');
    server.close();
  }
});
