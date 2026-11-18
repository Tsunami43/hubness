/**
 * hubness MCP server (stdio, line-delimited JSON-RPC 2.0). The agent gets the
 * open_pane tool: show a site preview, a file, or a diff next to the chat. The hub server
 * attaches this script to every Claude Code / Opencode session itself.
 *
 * Env: HUB_URL (http://127.0.0.1:8787), HUB_SESSION_ID.
 */
import { createInterface } from 'node:readline';

const HUB_URL = process.env.HUB_URL ?? 'http://127.0.0.1:8787';
const SESSION_ID = process.env.HUB_SESSION_ID ?? '';

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
];

type Json = Record<string, any>;
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
        return reply(m.id, text(`Unknown tool ${name}`, true));
      }
      default:
        return send({ jsonrpc: '2.0', id: m.id, error: { code: -32601, message: `Method not found: ${m.method}` } });
    }
  } catch (e) {
    reply(m.id, text(`Error: ${(e as Error).message}`, true));
  }
});
