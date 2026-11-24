import './setup.ts';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { isCloneUrl, parseRemote, repoNameFromUrl, resolveProvider, stripCredentials } from '../src/git-providers.ts';

test('parseRemote: ssh scp-style, ssh://, https with credentials', () => {
  assert.deepEqual(parseRemote('git@github.com:acme/auth-service.git'), { host: 'github.com', path: 'acme/auth-service' });
  assert.deepEqual(parseRemote('ssh://git@gitlab.example.com:2222/team/sub/billing.git'), { host: 'gitlab.example.com', path: 'team/sub/billing' });
  assert.deepEqual(parseRemote('https://user:token@bitbucket.org/acme/web'), { host: 'bitbucket.org', path: 'acme/web' });
  assert.equal(parseRemote('/home/me/repo'), null);
});

test('resolveProvider: GitHub, GitLab (self-hosted), Bitbucket, generic', () => {
  assert.equal(resolveProvider('git@github.com:acme/a.git')?.id, 'github');
  assert.equal(resolveProvider('https://gitlab.company.io/team/a.git')?.id, 'gitlab');
  assert.equal(resolveProvider('git@bitbucket.org:acme/a.git')?.id, 'bitbucket');
  const g = resolveProvider('ssh://git@git.internal:22/a.git');
  assert.equal(g?.id, 'generic');
  assert.equal(g?.adapter, null);
  assert.equal(resolveProvider(null), null);
});

test('provider links: branch, commit, new PR / MR', () => {
  const gh = resolveProvider('git@github.com:acme/auth.git')!;
  const l = gh.adapter!.links(gh.remote!);
  assert.equal(l.repo, 'https://github.com/acme/auth');
  assert.equal(l.branch('feat/x'), 'https://github.com/acme/auth/tree/feat/x');
  assert.equal(l.newPullRequest('fix', 'main'), 'https://github.com/acme/auth/compare/main...fix?expand=1');
  const gl = resolveProvider('https://gitlab.com/g/sub/svc.git')!;
  assert.match(gl.adapter!.links(gl.remote!).newPullRequest('fix', 'main'), /merge_requests\/new\?merge_request%5Bsource_branch%5D=fix&merge_request%5Btarget_branch%5D=main$/);
});

test('isCloneUrl rejects local paths, option injection and whitespace', () => {
  assert.equal(isCloneUrl('https://github.com/acme/a.git'), true);
  assert.equal(isCloneUrl('git@github.com:acme/a.git'), true);
  assert.equal(isCloneUrl('/etc'), false);
  assert.equal(isCloneUrl('-oProxyCommand=evil'), false);
  assert.equal(isCloneUrl('https://github.com/a b'), false);
  assert.equal(isCloneUrl('file:///tmp/repo'), false);
});

test('stripCredentials and repoNameFromUrl', () => {
  assert.equal(stripCredentials('https://user:secret@github.com/a/b.git'), 'https://github.com/a/b.git');
  assert.equal(stripCredentials('git@github.com:a/b.git'), 'git@github.com:a/b.git');
  assert.equal(repoNameFromUrl('git@github.com:acme/billing-service.git'), 'billing-service');
});
