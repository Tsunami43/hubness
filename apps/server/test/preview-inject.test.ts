/**
 * The helper the preview proxy adds to previewed pages (packages/shared/assets/preview-inject.js),
 * run in a bare `vm` context with the few browser objects it touches: what it reports to the
 * hub, and whom it talks to.
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import { setTimeout as sleep } from 'node:timers/promises';

const SOURCE = readFileSync(new URL('../../../packages/shared/assets/preview-inject.js', import.meta.url), 'utf8');

type Listener = (e: any) => void;

/** A page at http://hub.test:8790/app, optionally framed by a page of `parentOrigin`. */
function page(opts: { framed?: boolean; ancestor?: string; fetch?: (input: any, init?: any) => Promise<any> } = {}) {
  const beacons: any[][] = [];
  const posted: { msg: any; target: string }[] = [];
  const listeners: Record<string, Listener[]> = {};
  const docListeners: Record<string, Listener[]> = {};
  const logged: any[][] = [];
  const on = (map: Record<string, Listener[]>) => (name: string, fn: Listener) => void (map[name] ??= []).push(fn);
  const parent = { postMessage: (msg: any, target: string) => void posted.push({ msg: JSON.parse(JSON.stringify(msg)), target }) };
  const history = {
    back: () => void (history.went = 'back'),
    forward: () => void (history.went = 'forward'),
    pushState: (_s: unknown, _t: string, url: string) => void (location.pathname = url),
    replaceState: () => {},
    went: '',
  };
  const location: any = { href: 'http://hub.test:8790/app', hostname: 'hub.test', pathname: '/app', search: '', hash: '', reload: () => void (history.went = 'reload') };
  if (opts.ancestor) location.ancestorOrigins = [opts.ancestor];
  class XMLHttpRequest {
    status = 0;
    private handlers: Record<string, Listener[]> = {};
    open(_m: string, _u: string) {}
    send() {}
    addEventListener(name: string, fn: Listener) {
      (this.handlers[name] ??= []).push(fn);
    }
    emit(name: string) {
      for (const fn of this.handlers[name] ?? []) fn({});
    }
  }
  const window: any = {
    location,
    history,
    console: { error: (...a: any[]) => void logged.push(a), warn: (...a: any[]) => void logged.push(a), log: () => {} },
    navigator: {
      sendBeacon: (url: string, blob: Blob) => {
        void blob.text().then((t) => beacons.push([url, JSON.parse(t)]));
        return true;
      },
    },
    document: { title: 'App', addEventListener: on(docListeners), documentElement: { style: {}, appendChild() {} }, body: {}, createElement: () => ({ style: {} }) },
    addEventListener: on(listeners),
    fetch: opts.fetch,
    XMLHttpRequest,
    setTimeout,
    Blob,
    URL,
    JSON,
    String,
    Array,
    Error,
    Date,
  };
  window.window = window;
  window.parent = opts.framed ? parent : window;
  runInNewContext(SOURCE, window);
  return {
    window,
    beacons,
    posted,
    logged,
    history,
    /** A message event from the framing page. */
    message: (origin: string, data: object, source: object = parent) => listeners.message?.forEach((fn) => fn({ data, origin, source })),
    fire: (name: string, e: object) => listeners[name]?.forEach((fn) => fn(e)),
    flushed: () => sleep(360),
  };
}

test('helper: errors, warnings and failed loads go to the hub in one batch; the app’s console still works', async () => {
  const p = page();
  p.window.console.error('boom', { a: 1 }, new Error('bad'));
  p.window.console.warn('careful');
  p.fire('error', { message: 'x is not defined', filename: 'http://hub.test:8790/app.js', lineno: 7 });
  p.fire('error', { target: { src: 'http://hub.test:8790/missing.png' } });
  p.fire('unhandledrejection', { reason: 'nope' });
  assert.equal(p.logged.length, 2, 'the page’s own console got both calls');
  assert.equal(p.beacons.length, 0, 'nothing is sent per call');
  await p.flushed();
  assert.equal(p.beacons.length, 1);
  const [url, list] = p.beacons[0]!;
  assert.equal(url, '/__hubness/log');
  assert.deepEqual(list.map((e: any) => e.level), ['error', 'warn', 'error', 'network', 'error']);
  assert.match(list[0].text, /^boom \{"a":1\} Error: bad/);
  assert.equal(list[2].text, 'x is not defined (http://hub.test:8790/app.js:7)');
  assert.equal(list[3].text, 'Failed to load http://hub.test:8790/missing.png');
  assert.equal(list[4].text, 'Unhandled rejection: nope');
  assert.ok(list.every((e: any) => e.url === 'http://hub.test:8790/app' && /^\d{4}-/.test(e.ts)));
  assert.equal(p.posted.length, 0, 'not in a frame: nobody to talk to');
});

test('helper: more than the hub takes at once goes in several requests; a flood is cut', async () => {
  const p = page();
  for (let i = 0; i < 120; i++) p.window.console.warn(`w${i}`);
  await p.flushed();
  assert.deepEqual(p.beacons.map((b) => b[1].length), [50]);
  await sleep(700);
  assert.deepEqual(p.beacons.map((b) => b[1].length), [50, 50, 20], 'the rest follows without another error');
  assert.equal(p.beacons[2]![1].at(-1).text, 'w119');
  for (let i = 0; i < 1000; i++) p.window.console.warn('flood');
  await sleep(1500);
  const sent = p.beacons.slice(3).reduce((n, b) => n + b[1].length, 0);
  assert.ok(sent <= 250, `a runaway loop does not queue without end: ${sent}`);
});

test('helper: failed requests are reported, cancelled ones and the helper’s own are not', async () => {
  const answers: Record<string, () => Promise<any>> = {
    '/api/ok': async () => ({ status: 200 }),
    '/api/broken': async () => ({ status: 500 }),
    '/api/down': async () => {
      throw new TypeError('Failed to fetch');
    },
    '/api/cancelled': async () => {
      throw Object.assign(new Error('aborted'), { name: 'AbortError' });
    },
    '/__hubness/log': async () => ({ status: 500 }),
  };
  const p = page({ fetch: (input) => answers[typeof input === 'string' ? input : (input.url ?? input.pathname)]!() });
  const f = p.window.fetch;
  assert.equal((await f('/api/ok')).status, 200);
  assert.equal((await f('/api/broken', { method: 'POST' })).status, 500, 'the app still gets its answer');
  await assert.rejects(f('/api/down'), /Failed to fetch/);
  await assert.rejects(f('/api/cancelled'), /aborted/);
  await f('/__hubness/log');
  assert.equal((await f(new URL('http://hub.test:8790/api/broken'))).status, 500);
  assert.equal((await f({ url: '/api/broken', method: 'PUT' })).status, 500);

  const xhr = new p.window.XMLHttpRequest();
  xhr.open('GET', '/x/gone');
  xhr.send();
  xhr.status = 404;
  xhr.emit('loadend');
  const cancelled = new p.window.XMLHttpRequest();
  cancelled.open('GET', '/x/cancelled');
  cancelled.send();
  cancelled.emit('abort');
  cancelled.emit('loadend');
  const offline = new p.window.XMLHttpRequest();
  offline.open('POST', '/x/offline');
  offline.send();
  offline.emit('loadend');

  await p.flushed();
  assert.deepEqual(
    p.beacons.flatMap((b) => b[1]).map((e: any) => `${e.level}: ${e.text}`),
    [
      'network: POST /api/broken → 500',
      'network: GET /api/down failed: TypeError: Failed to fetch',
      'network: GET http://hub.test:8790/api/broken → 500',
      'network: PUT /api/broken → 500',
      'network: GET /x/gone → 404',
      'network: POST /x/offline failed',
    ].map((t) => t.replace('TypeError: Failed to fetch', p.beacons.flatMap((b) => b[1])[1].text.split('failed: ')[1])),
  );
  assert.match(p.beacons.flatMap((b) => b[1])[1].text, /Failed to fetch/);
});

test('helper: it talks only to a hubness panel on its own host', async () => {
  // framed by a foreign page (no ancestorOrigins, as in Firefox): silence until a trusted hello
  const p = page({ framed: true });
  p.window.console.error('secret detail');
  await p.flushed();
  assert.equal(p.posted.length, 0, 'nothing leaves before the panel is known');
  p.message('https://evil.example', { hubness: 1, type: 'hello' });
  p.message('https://evil.example', { hubness: 1, type: 'back' });
  p.message('https://evil.example', { hubness: 1, type: 'pick' });
  assert.equal(p.posted.length, 0);
  assert.equal(p.history.went, '', 'a foreign page commands nothing');
  p.message('http://hub.test:8787', { hubness: 1, type: 'hello' }, {});
  assert.equal(p.posted.length, 0, 'nor a message that is not from the parent window');
  p.message('http://hub.test:8787', { type: 'hello' });
  assert.equal(p.posted.length, 0, 'nor one without the mark');

  p.message('http://hub.test:8787', { hubness: 1, type: 'hello' });
  assert.ok(p.posted.length >= 2);
  assert.ok(p.posted.every((m) => m.target === 'http://hub.test:8787'), 'answers go to the panel’s origin, never to *');
  assert.ok(p.posted.some((m) => m.msg.type === 'ready'));
  assert.deepEqual(p.posted.find((m) => m.msg.type === 'location')!.msg, { type: 'location', path: '/app', title: 'App', hubness: 1 });
  assert.ok(!p.posted.some((m) => m.msg.type === 'log'), 'logs go to the hub, not through the frame');

  p.message('http://hub.test:8787', { hubness: 1, type: 'back' });
  assert.equal(p.history.went, 'back');
  p.message('http://hub.test:8787', { hubness: 1, type: 'forward' });
  assert.equal(p.history.went, 'forward');
  p.message('http://hub.test:8787', { hubness: 1, type: 'reload' });
  assert.equal(p.history.went, 'reload');
  p.message('https://evil.example', { hubness: 1, type: 'back' });
  assert.equal(p.history.went, 'reload', 'still nothing for strangers once the panel is known');

  // in-app navigation is reported
  const before = p.posted.length;
  p.window.history.pushState(null, '', '/cart');
  await sleep(10);
  assert.deepEqual(p.posted.slice(before).map((m) => m.msg.path), ['/cart']);
});

test('helper: where the browser names the frame’s ancestors, the panel is known at once — and a foreign one never', async () => {
  const mine = page({ framed: true, ancestor: 'http://hub.test:8787' });
  await sleep(10);
  assert.ok(mine.posted.some((m) => m.msg.type === 'ready' && m.target === 'http://hub.test:8787'));
  const foreign = page({ framed: true, ancestor: 'https://evil.example' });
  await sleep(10);
  assert.equal(foreign.posted.length, 0);
});

test('helper: loading it twice does nothing the second time', () => {
  const p = page();
  const wrapped = p.window.console.error;
  runInNewContext(SOURCE, p.window);
  assert.equal(p.window.console.error, wrapped);
});
