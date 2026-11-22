// A fake agent CLI for normalizer tests: replays recorded agent output (a fixture of
// stdout lines) so the real adapter parses it. REPLAY_MODE picks the protocol:
//  - stream: print every line after the first message arrives, exit when stdin closes (Claude Code stream-json);
//  - acp: JSON-RPC — for each client request emit the recorded messages up to the next
//    response (its id rewritten to the request's); agent requests wait for the client's answer.
import { readFileSync } from 'node:fs';
import { createInterface } from 'node:readline';

type Json = Record<string, any>;
const lines = readFileSync(process.env.REPLAY_FIXTURE!, 'utf8').split('\n').filter((l) => l.trim());
const out = (m: unknown) => process.stdout.write((typeof m === 'string' ? m : JSON.stringify(m)) + '\n');
const input = createInterface({ input: process.stdin });

if (process.env.REPLAY_MODE === 'stream') {
  let sent = false;
  input.on('line', () => {
    if (sent) return;
    sent = true;
    lines.forEach(out);
  });
  input.on('close', () => process.exit(0));
} else {
  const fixture = lines.map((l) => JSON.parse(l) as Json);
  let pos = 0;
  let waitingFor: number | null = null;
  let current: number | null = null;
  const pump = () => {
    while (pos < fixture.length && waitingFor === null && current !== null) {
      const m = fixture[pos++]!;
      if (m.method && m.id !== undefined) {
        out(m);
        waitingFor = m.id;
      } else if (m.method) out(m);
      else {
        out({ ...m, id: current });
        current = null;
      }
    }
  };
  input.on('line', (l) => {
    const m = JSON.parse(l) as Json;
    if (m.method && m.id !== undefined) {
      current = m.id;
      pump();
    } else if (!m.method && m.id === waitingFor) {
      waitingFor = null;
      pump();
    }
  });
  input.on('close', () => process.exit(0));
}
