import { makeRepo, tmp } from './setup.ts';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Hono } from 'hono';
import { api } from '../src/routes/api.ts';
import { HttpError } from '../src/errors.ts';
import { attachmentsNote, isModelImage, resolveAttachments, saveAttachment } from '../src/attachments.ts';

const app = new Hono();
app.onError((err, c) => c.json({ error: err.message }, err instanceof HttpError ? (err.status as 400) : 500));
app.route('/api', api);

const PNG = Uint8Array.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3]);

test('attachments: stored in the repository, excluded from git locally, resolved by id only', () => {
  const repo = makeRepo(join(tmp, 'attach-repo'));
  const a = saveAttachment(repo, '../../etc/passwd shot.png', 'image/png', PNG);
  assert.ok(a.path.startsWith(join(repo, '.hub', 'attachments') + '/'), 'inside the session folder');
  assert.ok(!a.name.includes('/'), 'no path in the name');
  assert.equal(a.mime, 'image/png');
  assert.ok(isModelImage(a));

  // git does not see the folder; the exclusion is local and written once
  saveAttachment(repo, 'second.txt', '', new TextEncoder().encode('hi'));
  const status = execFileSync('git', ['-C', repo, 'status', '--porcelain', '--untracked-files=all'], { encoding: 'utf8' });
  assert.ok(!status.includes('attachments'), status);
  const exclude = readFileSync(join(repo, '.git', 'info', 'exclude'), 'utf8');
  assert.equal(exclude.split('\n').filter((l) => l === '/.hub/attachments/').length, 1);
  assert.ok(!existsSync(join(repo, '.gitignore')), '.gitignore untouched');

  const [r] = resolveAttachments(repo, [{ id: a.id, name: a.name, mime: a.mime }]);
  assert.equal(r!.path, a.path);
  assert.throws(() => resolveAttachments(repo, [{ id: '../x' }]), /not found/);
  assert.throws(() => resolveAttachments(repo, [{ id: 'missing.png' }]), /not found/);
  assert.match(attachmentsNote([a]), /Attached files[\s\S]*\.png \(image\/png, 1 KB\)/);
  assert.throws(() => saveAttachment(repo, 'empty', '', new Uint8Array()), /Empty/);
});

test('attachments over HTTP: upload into the chat folder, serve images inline, others as a download', async () => {
  const s = await app.request('/api/sessions', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ title: 'with files', agent: { kind: 'claude-code' } }),
  });
  const session = (await s.json()) as { id: string; cwd: string };
  const up = await app.request(`/api/sessions/${session.id}/attachments`, {
    method: 'POST',
    headers: { 'X-Hubness-Request': '1', 'Content-Type': 'image/png', 'X-Filename': encodeURIComponent('скрин 1.png') },
    body: PNG,
  });
  assert.equal(up.status, 201);
  const a = (await up.json()) as { id: string; name: string; path: string };
  assert.equal(a.name, 'скрин 1.png');
  assert.ok(a.path.startsWith(session.cwd));

  const img = await app.request(`/api/sessions/${session.id}/attachments/${encodeURIComponent(a.id)}`);
  assert.equal(img.headers.get('content-type'), 'image/png');
  assert.equal(img.headers.get('content-disposition'), 'inline');
  assert.deepEqual(new Uint8Array(await img.arrayBuffer()), PNG);

  const html = await app.request(`/api/sessions/${session.id}/attachments`, {
    method: 'POST',
    headers: { 'X-Hubness-Request': '1', 'Content-Type': 'text/html', 'X-Filename': 'page.html' },
    body: '<script>alert(1)</script>',
  });
  const h = (await html.json()) as { id: string };
  const served = await app.request(`/api/sessions/${session.id}/attachments/${encodeURIComponent(h.id)}`);
  assert.equal(served.headers.get('content-type'), 'application/octet-stream', 'never served as a page');
  assert.equal(served.headers.get('content-disposition'), 'attachment');

  assert.equal((await app.request(`/api/sessions/${session.id}/attachments/..%2F..%2Fsecret`)).status, 404);
});
