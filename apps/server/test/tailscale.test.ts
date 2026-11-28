/**
 * Previews behind Tailscale Serve: the hub adds a Serve route for a preview port and removes
 * it again (a stand-in `tailscale` records what it was asked).
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { chmodSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { exposePreview, initTailscale, serveFront, unexposeAllPreviews, unexposePreview } from '../src/tailscale.ts';

const status = (proxy: string, https: boolean) => ({
  TCP: { '443': https ? { HTTPS: true } : { HTTP: true } },
  Web: { 'box.tail1234.ts.net:443': { Handlers: { '/': { Proxy: proxy } } } },
});

test('the hub is behind Serve only when a route proxies to its port', () => {
  assert.deepEqual(serveFront(status('http://127.0.0.1:8787', true), 8787), { https: true });
  assert.deepEqual(serveFront(status('http://localhost:8787/', false), 8787), { https: false });
  assert.equal(serveFront(status('http://127.0.0.1:3000', true), 8787), null, 'another app');
  assert.equal(serveFront(status('http://10.0.0.5:8787', true), 8787), null, 'another machine');
  assert.equal(serveFront(status('/srv/files', true), 8787), null);
  assert.equal(serveFront({}, 8787), null);
  assert.equal(serveFront(null, 8787), null);
});

test('preview ports get a Serve route and lose it', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'hub-ts-'));
  const log = join(dir, 'calls.log');
  const tool = join(dir, 'tailscale');
  writeFileSync(
    tool,
    `#!/bin/sh\necho "$*" >> '${log}'\nif [ "$1 $2" = 'serve status' ]; then echo '${JSON.stringify(status('http://127.0.0.1:61000', true))}'; fi\ncase "$*" in *=61002*) echo 'Access denied: serve config denied' >&2; exit 1;; esac\n`,
  );
  chmodSync(tool, 0o755);
  const calls = (port: string) => {
    try {
      return readFileSync(log, 'utf8').split('\n').filter((l) => l.includes(port));
    } catch {
      return [];
    }
  };

  // not started by a server: nothing runs
  await exposePreview(61001);
  assert.deepEqual(calls('61001'), []);

  process.env.HUBNESS_TAILSCALE = tool;
  initTailscale(61000, '127.0.0.1');
  try {
    void exposePreview(61001);
    await exposePreview(61001);
    assert.deepEqual(calls('61001'), ['serve --bg --https=61001 http://127.0.0.1:61001'], 'once');
    void unexposePreview(61001);
    await unexposePreview(61001);
    assert.deepEqual(calls('61001').slice(1), ['serve --https=61001 off']);

    // refused (no operator rights): not ours, tried again when asked again, never removed
    const warn = console.warn;
    const warnings: string[] = [];
    console.warn = (m: string) => void warnings.push(m);
    try {
      await exposePreview(61002);
      await exposePreview(61002);
      await unexposePreview(61002);
    } finally {
      console.warn = warn;
    }
    assert.deepEqual(calls('61002'), Array(2).fill('serve --bg --https=61002 http://127.0.0.1:61002'));
    assert.equal(warnings.length, 1);
    assert.match(warnings[0]!, /tailscale set --operator/);

    await exposePreview(61003);
    unexposeAllPreviews();
    assert.deepEqual(calls('61003').slice(1), ['serve --https=61003 off']);

    process.env.HUBNESS_TAILSCALE = '0';
    await exposePreview(61004);
    assert.deepEqual(calls('61004'), [], 'turned off');
  } finally {
    delete process.env.HUBNESS_TAILSCALE;
  }
});
