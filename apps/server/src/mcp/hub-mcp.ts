/**
 * hubness MCP server (stdio, line-delimited JSON-RPC 2.0). The agent gets:
 * - open_pane / close_pane: show a site preview, a file, or a diff next to the chat;
 * - preview_*: its own headless browser (on the hub) to open the app it builds, look at
 *   screenshots, click, type, run JavaScript and read the console — to check its work.
 * The hub server attaches this script to every agent session itself. The Rust `hubness mcp`
 * has the same tools (crates/server/src/mcp.rs).
 *
 * Env: HUB_URL (http://127.0.0.1:8787), HUB_SESSION_ID.
 */
import { createInterface } from 'node:readline';

const HUB_URL = process.env.HUB_URL ?? 'http://127.0.0.1:8787';
const SESSION_ID = process.env.HUB_SESSION_ID ?? '';

const shotProp = { screenshot: { type: 'boolean', description: 'Attach a screenshot of the page after the action' } };
/** The agent's browser: each tool is one POST /api/sessions/:id/browser action. */
const PREVIEW_TOOLS = [
  {
    name: 'preview_open',
    description:
      'Open a page in your own headless browser on the hub to check a web app you are building or running ' +
      '(e.g. http://localhost:5173/settings — localhost works, also for apps in the project container). ' +
      'Returns the title, console errors and failed requests, and a screenshot unless screenshot=false. ' +
      'Use it after changing UI code to see the result yourself instead of asking the user. ' +
      'width/height set the window (e.g. 390×844 to check the phone layout).',
    inputSchema: {
      type: 'object',
      properties: {
        url: { type: 'string', description: 'Address to open' },
        width: { type: 'number', description: 'Window width in CSS pixels (default 1280; under 600 emulates a phone)' },
        height: { type: 'number', description: 'Window height (default 800)' },
        screenshot: { type: 'boolean', description: 'Attach a screenshot (default true)' },
      },
      required: ['url'],
    },
  },
  {
    name: 'preview_screenshot',
    description: 'Screenshot of the page open in your preview browser.',
    inputSchema: { type: 'object', properties: { fullPage: { type: 'boolean', description: 'The whole scrollable page, not only the window' } } },
  },
  {
    name: 'preview_click',
    description: 'Click an element in the preview browser, found by CSS selector or by its visible text (buttons, links, labels).',
    inputSchema: { type: 'object', properties: { selector: { type: 'string' }, text: { type: 'string', description: 'Visible text of the element' }, ...shotProp } },
  },
  {
    name: 'preview_type',
    description: 'Type into a field in the preview browser (replaces its value). Find it by CSS selector or by text (label / placeholder).',
    inputSchema: {
      type: 'object',
      properties: { selector: { type: 'string' }, text: { type: 'string', description: 'Label, placeholder or visible text' }, value: { type: 'string', description: 'What to type' }, ...shotProp },
      required: ['value'],
    },
  },
  {
    name: 'preview_press',
    description: 'Press a key in the preview browser: Enter, Tab, Escape, Backspace, ArrowDown… or one character.',
    inputSchema: { type: 'object', properties: { key: { type: 'string' }, ...shotProp }, required: ['key'] },
  },
  {
    name: 'preview_eval',
    description: 'Run a JavaScript expression in the preview page (awaited) and get its JSON value — read the DOM, app state, localStorage…',
    inputSchema: { type: 'object', properties: { expression: { type: 'string' } }, required: ['expression'] },
  },
  {
    name: 'preview_console',
    description:
      'New console messages, uncaught errors and failed requests of the preview page since the last call — ' +
      'yours and those the user saw in the hubness Preview panel for the same app.',
    inputSchema: { type: 'object', properties: {} },
  },
  {
    name: 'preview_close',
    description: 'Close your preview browser page.',
    inputSchema: { type: 'object', properties: {} },
  },
];

const TOOLS = [
  {
    name: 'open_pane',
    description:
      'Show something to the user in a panel next to the chat in the hubness UI, instead of describing it in text. ' +
      'Use it to show a running web app or dev server (kind=url, e.g. http://localhost:5173), a dashboard (Grafana), ' +
      'a file you created or changed (kind=file, path relative to the working directory), or the git diff of a file (kind=diff). ' +
      'The panel replaces the previous one.',
    inputSchema: {
      type: 'object',
      properties: {
        kind: { type: 'string', enum: ['url', 'file', 'diff'], description: 'What to show' },
        target: { type: 'string', description: 'URL for kind=url; file path for kind=file or kind=diff' },
        title: { type: 'string', description: 'Short title for the panel header' },
      },
      required: ['kind', 'target'],
    },
  },
  {
    name: 'close_pane',
    description: 'Close the panel next to the chat in the hubness UI.',
    inputSchema: { type: 'object', properties: {} },
  },
  ...PREVIEW_TOOLS,
];

type Json = Record<string, any>;

async function browser(args: Json, action: string) {
  if (!SESSION_ID) return text('hubness session is not set', true);
  const body: Json = { action };
  for (const k of ['url', 'width', 'height', 'selector', 'text', 'value', 'key', 'expression', 'fullPage', 'screenshot']) if (args[k] !== undefined) body[k] = args[k];
  if (action === 'open' && body.screenshot === undefined) body.screenshot = true;
  const res = await fetch(`${HUB_URL}/api/sessions/${SESSION_ID}/browser`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  const r = (await res.json().catch(() => ({}))) as Json;
  if (!res.ok) return text(r.error ?? `hub responded ${res.status}`, true);
  const lines = [r.text, r.url ? `Page: ${r.title ? `${r.title} — ` : ''}${r.url}` : ''].filter(Boolean);
  if (r.logs?.length) {
    lines.push('', 'Console / errors:');
    for (const l of r.logs) lines.push(`[${l.source === 'user' ? 'user ' : ''}${l.level}] ${l.text}`);
  } else if (action !== 'close') lines.push('No console errors.');
  const content: Json[] = [{ type: 'text', text: lines.join('\n') }];
  if (r.screenshot) content.push({ type: 'image', data: r.screenshot, mimeType: 'image/png' });
  return { content, isError: false };
}
const send = (msg: Json) => process.stdout.write(JSON.stringify(msg) + '\n');
const reply = (id: unknown, result: Json) => send({ jsonrpc: '2.0', id, result });
const text = (t: string, isError = false) => ({ content: [{ type: 'text', text: t }], isError });

async function callHub(body: Json | null): Promise<string> {
  if (!SESSION_ID) return 'hubness session is not set';
  const res = await fetch(`${HUB_URL}/api/sessions/${SESSION_ID}/pane`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ pane: body }),
  });
  if (!res.ok) throw new Error(`hub responded ${res.status}: ${await res.text()}`);
  return 'ok';
}

createInterface({ input: process.stdin }).on('line', async (line) => {
  let m: Json;
  try {
    m = JSON.parse(line);
  } catch {
    return;
  }
  if (m.id === undefined) return; // notifications (initialized, etc.)
  try {
    switch (m.method) {
      case 'initialize':
        return reply(m.id, {
          protocolVersion: m.params?.protocolVersion ?? '2025-06-18',
          capabilities: { tools: {} },
          serverInfo: { name: 'hubness', version: '0.1.0' },
        });
      case 'ping':
        return reply(m.id, {});
      case 'tools/list':
        return reply(m.id, { tools: TOOLS });
      case 'tools/call': {
        const { name, arguments: args = {} } = m.params ?? {};
        if (name === 'open_pane') {
          if (!['url', 'file', 'diff'].includes(args.kind) || !args.target) return reply(m.id, text('kind and target are required', true));
          await callHub({ kind: args.kind, target: String(args.target), title: args.title ? String(args.title) : undefined });
          return reply(m.id, text(`Opened ${args.kind} panel: ${args.target}`));
        }
        if (name === 'close_pane') {
          await callHub(null);
          return reply(m.id, text('Panel closed'));
        }
        if (typeof name === 'string' && name.startsWith('preview_')) return reply(m.id, await browser(args, name.slice('preview_'.length)));
        return reply(m.id, text(`Unknown tool ${name}`, true));
      }
      default:
        return send({ jsonrpc: '2.0', id: m.id, error: { code: -32601, message: `Method not found: ${m.method}` } });
    }
  } catch (e) {
    reply(m.id, text(`Error: ${(e as Error).message}`, true));
  }
});
