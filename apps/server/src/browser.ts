/**
 * The agent's own browser (preview_* tools of the hub MCP): a headless Chromium the hub
 * starts on first use, one page per session. The agent opens the app it is building, looks
 * at screenshots, clicks, types and reads the console — so it checks its work itself.
 *
 * Chromium is driven with the DevTools protocol over a pipe (`--remote-debugging-pipe`:
 * fd 3 in, fd 4 out, NUL-separated JSON), not a debugging port — no other local process can
 * take it over, and no WebSocket client is needed. The Rust server does the same in
 * `browser.rs`.
 *
 * http://localhost:<port> of a project session goes through the project's preview proxy:
 * the app works from a container too, and the agent sees the same page (with the preview
 * helper) as the user. The browser closes after BROWSER_IDLE_MS without use.
 */
import { spawn, type ChildProcess } from 'node:child_process';
import { mkdirSync, statSync } from 'node:fs';
import { delimiter, join } from 'node:path';
import type { Readable, Writable } from 'node:stream';
import type { BrowserRequest, BrowserResult, PreviewLog } from '@hubness/shared';
import { childEnv, config } from './config.ts';
import { HttpError } from './errors.ts';
import { AGENT_UA_MARK, previewKeyByPort, previewLogs } from './preview.ts';

const BROWSER_IDLE_MS = 10 * 60_000;
const NAV_TIMEOUT_MS = 20_000;
const CANDIDATES = ['chromium', 'chromium-browser', 'google-chrome', 'google-chrome-stable', 'chrome', 'brave', 'brave-browser', 'microsoft-edge'];

/** HUBNESS_CHROME, else the first Chromium-like browser on PATH. */
export function findChrome(): string | null {
  const env = process.env.HUBNESS_CHROME;
  if (env) return env;
  for (const dir of (process.env.PATH ?? '').split(delimiter).filter(Boolean))
    for (const name of CANDIDATES) {
      try {
        if (statSync(join(dir, name)).isFile()) return join(dir, name);
      } catch {}
    }
  return null;
}

type Json = Record<string, any>;

/** One DevTools connection over the pipe; commands of a page carry its sessionId. */
class Cdp {
  private id = 0;
  private pending = new Map<number, { resolve: (v: Json) => void; reject: (e: Error) => void }>();
  private buf = '';
  listeners = new Set<(method: string, params: Json, sessionId?: string) => void>();
  closed = false;

  private input: Writable;

  constructor(input: Writable, output: Readable) {
    this.input = input;
    output.on('data', (d: Buffer) => {
      this.buf += d.toString('utf8');
      let i: number;
      while ((i = this.buf.indexOf('\0')) >= 0) {
        const raw = this.buf.slice(0, i);
        this.buf = this.buf.slice(i + 1);
        let m: Json;
        try {
          m = JSON.parse(raw);
        } catch {
          continue;
        }
        if (typeof m.id === 'number') {
          const p = this.pending.get(m.id);
          this.pending.delete(m.id);
          if (m.error) p?.reject(new Error(m.error.message ?? 'DevTools error'));
          else p?.resolve(m.result ?? {});
        } else if (m.method) for (const l of this.listeners) l(m.method, m.params ?? {}, m.sessionId);
      }
    });
    output.on('close', () => this.fail());
    input.on('error', () => this.fail());
  }

  private fail() {
    this.closed = true;
    for (const p of this.pending.values()) p.reject(new Error('The browser closed'));
    this.pending.clear();
  }

  send(method: string, params: Json = {}, sessionId?: string, timeout = NAV_TIMEOUT_MS): Promise<Json> {
    if (this.closed) return Promise.reject(new Error('The browser closed'));
    const id = ++this.id;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method} timed out`));
      }, timeout);
      this.pending.set(id, {
        resolve: (v) => {
          clearTimeout(timer);
          resolve(v);
        },
        reject: (e) => {
          clearTimeout(timer);
          reject(e);
        },
      });
      this.input.write(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }) + '\0');
    });
  }
}

interface Page {
  sessionId: string;
  targetId: string;
  /** Logs of this page not yet returned to the agent. */
  logs: PreviewLog[];
  seq: number;
  /** The preview key the page is on (for the user's logs) and how far they were read. */
  previewKey: string | null;
  userSeen: number;
  /** localhost:<app port> ⇄ 127.0.0.1:<preview port>: the agent sees its own address. */
  alias: { from: string; to: string } | null;
  loaded: (() => void) | null;
}

let chrome: { proc: ChildProcess; cdp: Cdp } | null = null;
let starting: Promise<{ proc: ChildProcess; cdp: Cdp }> | null = null;
const pages = new Map<string, Page>();
let idle: ReturnType<typeof setTimeout> | null = null;

function touch() {
  if (idle) clearTimeout(idle);
  idle = setTimeout(() => shutdownBrowser(), BROWSER_IDLE_MS);
  idle.unref?.();
}

async function browser(): Promise<{ proc: ChildProcess; cdp: Cdp }> {
  if (chrome && !chrome.cdp.closed) return chrome;
  if (starting) return starting;
  starting = (async () => {
    const bin = findChrome();
    if (!bin) throw new HttpError(503, 'No Chromium or Chrome found on the hub machine. Install chromium (or set HUBNESS_CHROME) to let agents use a browser.');
    const profile = join(config.dataDir, 'browser');
    mkdirSync(profile, { recursive: true });
    const proc = spawn(
      bin,
      [
        '--headless=new',
        '--remote-debugging-pipe',
        `--user-data-dir=${profile}`,
        '--no-first-run',
        '--no-default-browser-check',
        '--disable-gpu',
        '--disable-dev-shm-usage',
        '--hide-scrollbars',
        '--mute-audio',
        'about:blank',
      ],
      { env: childEnv(), stdio: ['ignore', 'ignore', 'ignore', 'pipe', 'pipe'] },
    );
    const cdp = new Cdp(proc.stdio[3] as Writable, proc.stdio[4] as Readable);
    proc.on('exit', () => {
      cdp.closed = true;
      pages.clear();
      if (chrome?.proc === proc) chrome = null;
    });
    await new Promise<void>((resolve, reject) => {
      proc.once('error', (e) => reject(new HttpError(503, `Chromium didn't start: ${e.message}`)));
      cdp.send('Browser.getVersion', {}, undefined, 15_000).then(() => resolve(), (e) => reject(new HttpError(503, `Chromium didn't start: ${(e as Error).message}`)));
    });
    cdp.listeners.add(onEvent);
    chrome = { proc, cdp };
    return chrome;
  })();
  try {
    return await starting;
  } finally {
    starting = null;
  }
}

function pageBySession(sessionId: string | undefined): Page | undefined {
  for (const p of pages.values()) if (p.sessionId === sessionId) return p;
  return undefined;
}

function pushLog(p: Page, level: PreviewLog['level'], text: string, url = '') {
  p.logs.push({ seq: ++p.seq, ts: new Date().toISOString(), level, text: text.slice(0, 4000), url, source: 'agent' });
  if (p.logs.length > 200) p.logs.splice(0, p.logs.length - 200);
}

const remote = (o: Json) => (o.value !== undefined ? (typeof o.value === 'string' ? o.value : JSON.stringify(o.value)) : (o.description ?? o.type ?? ''));

function onEvent(method: string, params: Json, sessionId?: string) {
  const p = pageBySession(sessionId);
  if (!p) return;
  switch (method) {
    case 'Runtime.consoleAPICalled': {
      const level = params.type === 'error' || params.type === 'assert' ? 'error' : params.type === 'warning' ? 'warn' : 'log';
      pushLog(p, level, (params.args ?? []).map(remote).join(' '), params.stackTrace?.callFrames?.[0]?.url ?? '');
      break;
    }
    case 'Runtime.exceptionThrown': {
      const d = params.exceptionDetails ?? {};
      pushLog(p, 'error', d.exception?.description ?? d.text ?? 'Uncaught exception', d.url ?? '');
      break;
    }
    case 'Log.entryAdded':
      // network errors come from Network.* (with the address) — not twice
      if (params.entry?.level === 'error' && params.entry.source !== 'network') pushLog(p, 'error', params.entry.text ?? '', params.entry.url ?? '');
      break;
    case 'Network.responseReceived': {
      const r = params.response ?? {};
      if (r.status >= 400 && !String(r.url).includes('/__hubness/')) pushLog(p, 'network', `${r.status} ${r.url}`, r.url);
      break;
    }
    case 'Network.loadingFailed':
      if (!params.canceled) pushLog(p, 'network', `Request failed: ${params.errorText ?? ''}`);
      break;
    case 'Page.loadEventFired':
      p.loaded?.();
      p.loaded = null;
      break;
  }
}

async function pageFor(hubSession: string): Promise<{ cdp: Cdp; page: Page }> {
  const { cdp } = await browser();
  touch();
  const cur = pages.get(hubSession);
  if (cur) return { cdp, page: cur };
  const { targetId } = await cdp.send('Target.createTarget', { url: 'about:blank' });
  const { sessionId } = await cdp.send('Target.attachToTarget', { targetId, flatten: true });
  const page: Page = { sessionId, targetId, logs: [], seq: 0, previewKey: null, userSeen: 0, alias: null, loaded: null };
  pages.set(hubSession, page);
  const { userAgent } = await cdp.send('Browser.getVersion');
  await Promise.all([
    cdp.send('Page.enable', {}, sessionId),
    cdp.send('Runtime.enable', {}, sessionId),
    cdp.send('Log.enable', {}, sessionId),
    cdp.send('Network.enable', {}, sessionId),
    cdp.send('Network.setUserAgentOverride', { userAgent: `${String(userAgent).replace('HeadlessChrome', 'Chrome')} ${AGENT_UA_MARK}` }, sessionId),
  ]);
  await viewport(cdp, page, 1280, 800);
  return { cdp, page };
}

async function viewport(cdp: Cdp, page: Page, width: number, height: number) {
  const w = Math.min(3840, Math.max(240, Math.round(width)));
  const h = Math.min(4000, Math.max(240, Math.round(height)));
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: w, height: h, deviceScaleFactor: 1, mobile: w < 600 }, page.sessionId);
  await cdp.send('Emulation.setTouchEmulationEnabled', { enabled: w < 600 }, page.sessionId);
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Run an expression in the page; the value comes back as JSON. */
async function evaluate(cdp: Cdp, page: Page, expression: string): Promise<any> {
  const r = await cdp.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true, userGesture: true }, page.sessionId);
  if (r.exceptionDetails) throw new HttpError(400, r.exceptionDetails.exception?.description ?? r.exceptionDetails.text ?? 'The expression threw');
  return r.result?.value;
}

/** The page's address as the agent wrote it (the preview proxy port mapped back). */
const shown = (page: Page, url: string) => (page.alias && url.startsWith(page.alias.to) ? page.alias.from + url.slice(page.alias.to.length) : url);

/** Where an element is (center, in CSS pixels), scrolled into view; by selector or visible text. */
async function locate(cdp: Cdp, page: Page, b: BrowserRequest): Promise<{ x: number; y: number; desc: string }> {
  const find = b.selector
    ? `document.querySelector(${JSON.stringify(b.selector)})`
    : `(() => { const want = ${JSON.stringify((b.text ?? '').trim().toLowerCase())};
        const all = [...document.querySelectorAll('a,button,input,select,textarea,label,summary,[role=button],[role=link],[role=tab],[role=menuitem],[onclick],[tabindex]')];
        const vis = (e) => { const r = e.getBoundingClientRect(); return r.width > 0 && r.height > 0; };
        const txt = (e) => (e.innerText || e.value || e.getAttribute('aria-label') || e.getAttribute('placeholder') || e.title || '').trim().toLowerCase();
        return all.find((e) => vis(e) && txt(e) === want) || all.find((e) => vis(e) && txt(e).includes(want))
          || [...document.querySelectorAll('body *')].find((e) => vis(e) && e.children.length === 0 && (e.textContent || '').trim().toLowerCase().includes(want)) || null; })()`;
  const at = await evaluate(
    cdp,
    page,
    `(() => { const e = ${find}; if (!e) return null; e.scrollIntoView({ block: 'center', inline: 'center' });
      const r = e.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2,
      desc: e.tagName.toLowerCase() + (e.id ? '#' + e.id : '') + ' "' + (e.innerText || e.value || '').trim().slice(0, 60) + '"' }; })()`,
  );
  if (!at) throw new HttpError(404, b.selector ? `No element matches ${b.selector}` : `No element with the text “${b.text}”`);
  return at;
}

async function waitSettled(page: Page, ms = 600) {
  // a click or a key may start a navigation: give it a moment, then wait for its load
  await sleep(150);
  if (page.loaded) await Promise.race([new Promise<void>((r) => (page.loaded = r)), sleep(ms)]);
  else await sleep(ms);
}

const KEYS: Record<string, { key: string; code: string; keyCode: number; text?: string }> = {
  Enter: { key: 'Enter', code: 'Enter', keyCode: 13, text: '\r' },
  Tab: { key: 'Tab', code: 'Tab', keyCode: 9 },
  Escape: { key: 'Escape', code: 'Escape', keyCode: 27 },
  Backspace: { key: 'Backspace', code: 'Backspace', keyCode: 8 },
  Delete: { key: 'Delete', code: 'Delete', keyCode: 46 },
  Space: { key: ' ', code: 'Space', keyCode: 32, text: ' ' },
  ArrowUp: { key: 'ArrowUp', code: 'ArrowUp', keyCode: 38 },
  ArrowDown: { key: 'ArrowDown', code: 'ArrowDown', keyCode: 40 },
  ArrowLeft: { key: 'ArrowLeft', code: 'ArrowLeft', keyCode: 37 },
  ArrowRight: { key: 'ArrowRight', code: 'ArrowRight', keyCode: 39 },
  Home: { key: 'Home', code: 'Home', keyCode: 36 },
  End: { key: 'End', code: 'End', keyCode: 35 },
  PageUp: { key: 'PageUp', code: 'PageUp', keyCode: 33 },
  PageDown: { key: 'PageDown', code: 'PageDown', keyCode: 34 },
};

/**
 * Route http://localhost:<port> of a project session through its preview proxy. `preview`
 * gives the preview port for an app port (it starts the proxy when needed).
 */
export type PreviewFor = (port: number) => Promise<number | null>;

async function navigate(cdp: Cdp, page: Page, url: string, previewFor: PreviewFor | null) {
  let target = url.trim();
  // `localhost:5173/x` has no scheme ("localhost:" is not one)
  if (!/^([a-z][a-z0-9+.-]*:\/\/|about:)/i.test(target)) target = `http://${target}`;
  let u: URL;
  try {
    u = new URL(target);
  } catch {
    throw new HttpError(400, `Not a URL: ${url}`);
  }
  if (!['http:', 'https:', 'file:'].includes(u.protocol) && target !== 'about:blank') throw new HttpError(400, 'Only http(s) and file addresses');
  page.alias = null;
  page.previewKey = null;
  if (previewFor && u.protocol === 'http:' && ['localhost', '127.0.0.1', '0.0.0.0', '[::1]'].includes(u.hostname) && u.port) {
    const pp = await previewFor(Number(u.port)).catch(() => null);
    if (pp) {
      const from = `${u.protocol}//${u.host}`;
      const to = `http://127.0.0.1:${pp}`;
      page.alias = { from, to };
      page.previewKey = previewKeyByPort(pp);
      page.userSeen = page.previewKey ? (previewLogs(page.previewKey).at(-1)?.seq ?? 0) : 0;
      target = to + target.slice(from.length);
    }
  }
  const loaded = new Promise<void>((r) => (page.loaded = r));
  const r = await cdp.send('Page.navigate', { url: target }, page.sessionId);
  if (r.errorText) {
    page.loaded = null;
    throw new HttpError(502, `Can't open ${url}: ${r.errorText}`);
  }
  await Promise.race([loaded, sleep(NAV_TIMEOUT_MS)]);
  page.loaded = null;
  // client-side apps render after load
  await sleep(500);
}

async function screenshot(cdp: Cdp, page: Page, fullPage: boolean): Promise<string> {
  let clip: Json | undefined;
  if (fullPage) {
    const m = await cdp.send('Page.getLayoutMetrics', {}, page.sessionId);
    const size = m.cssContentSize ?? m.contentSize;
    clip = { x: 0, y: 0, width: Math.ceil(size.width), height: Math.min(8000, Math.ceil(size.height)), scale: 1 };
  }
  const r = await cdp.send('Page.captureScreenshot', { format: 'png', ...(clip ? { clip, captureBeyondViewport: true } : {}) }, page.sessionId);
  return r.data as string;
}

/** One agent browser action for a hub session. */
export async function browserAction(hubSession: string, b: BrowserRequest, previewFor: PreviewFor | null): Promise<BrowserResult> {
  if (b.action === 'close') {
    await closeSessionBrowser(hubSession);
    return { ok: true, url: '', title: '', text: 'Browser page closed', logs: [] };
  }
  if (b.action === 'console' && !pages.has(hubSession)) return { ok: true, url: '', title: '', text: 'No page open yet — use preview_open first', logs: [] };
  const { cdp, page } = await pageFor(hubSession);
  let text = '';
  let shot: string | undefined;
  switch (b.action) {
    case 'open':
      if (!b.url) throw new HttpError(400, 'url is required');
      if (b.width || b.height) await viewport(cdp, page, b.width ?? 1280, b.height ?? 800);
      await navigate(cdp, page, b.url, previewFor);
      text = 'Opened';
      break;
    case 'screenshot':
      shot = await screenshot(cdp, page, Boolean(b.fullPage));
      text = 'Screenshot taken';
      break;
    case 'click': {
      if (!b.selector && !b.text) throw new HttpError(400, 'selector or text is required');
      const at = await locate(cdp, page, b);
      for (const type of ['mouseMoved', 'mousePressed', 'mouseReleased'])
        await cdp.send('Input.dispatchMouseEvent', { type, x: at.x, y: at.y, button: 'left', clickCount: type === 'mouseMoved' ? 0 : 1 }, page.sessionId);
      await waitSettled(page);
      text = `Clicked ${at.desc}`;
      break;
    }
    case 'type': {
      if (!b.selector && !b.text) throw new HttpError(400, 'selector or text is required');
      const at = await locate(cdp, page, b);
      await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: at.x, y: at.y, button: 'left', clickCount: 1 }, page.sessionId);
      await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: at.x, y: at.y, button: 'left', clickCount: 1 }, page.sessionId);
      // replace the value: select everything in the focused field, then type over it
      await evaluate(cdp, page, `(() => { const e = document.activeElement; if (e && 'select' in e) e.select(); else document.execCommand('selectAll'); })()`);
      await cdp.send('Input.insertText', { text: b.value ?? '' }, page.sessionId);
      await waitSettled(page, 300);
      text = `Typed into ${at.desc}`;
      break;
    }
    case 'press': {
      const k = KEYS[b.key ?? ''] ?? (b.key && b.key.length === 1 ? { key: b.key, code: '', keyCode: b.key.toUpperCase().charCodeAt(0), text: b.key } : null);
      if (!k) throw new HttpError(400, `Unknown key ${b.key}; use ${Object.keys(KEYS).join(', ')} or one character`);
      await cdp.send('Input.dispatchKeyEvent', { type: k.text ? 'keyDown' : 'rawKeyDown', key: k.key, code: k.code, windowsVirtualKeyCode: k.keyCode, text: k.text }, page.sessionId);
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: k.key, code: k.code, windowsVirtualKeyCode: k.keyCode }, page.sessionId);
      await waitSettled(page);
      text = `Pressed ${k.key}`;
      break;
    }
    case 'eval': {
      if (!b.expression) throw new HttpError(400, 'expression is required');
      const v = await evaluate(cdp, page, b.expression);
      text = v === undefined ? 'undefined' : JSON.stringify(v, null, 2).slice(0, 20_000);
      break;
    }
    case 'console':
      text = 'Console';
      break;
    default:
      throw new HttpError(400, 'Unknown action');
  }
  if (b.screenshot && !shot) shot = await screenshot(cdp, page, false);
  const info = await evaluate(cdp, page, '({ url: location.href, title: document.title })').catch(() => ({ url: '', title: '' }));
  const logs = page.logs.splice(0);
  if (page.previewKey) {
    const user = previewLogs(page.previewKey, page.userSeen).filter((l) => l.source === 'user');
    if (user.length) page.userSeen = user.at(-1)!.seq;
    logs.push(...user.slice(-50));
  }
  touch();
  return { ok: true, url: shown(page, info.url ?? ''), title: info.title ?? '', text, ...(shot ? { screenshot: shot } : {}), logs };
}

export async function closeSessionBrowser(hubSession: string) {
  const p = pages.get(hubSession);
  if (!p) return;
  pages.delete(hubSession);
  await chrome?.cdp.send('Target.closeTarget', { targetId: p.targetId }).catch(() => {});
}

export function shutdownBrowser() {
  pages.clear();
  chrome?.proc.kill();
  chrome = null;
}
