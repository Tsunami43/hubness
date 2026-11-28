import { spawn } from 'node:child_process';
import { tmpdir } from 'node:os';
import type { ModelOption } from '@hubness/shared';
import { childEnv } from '../../config.ts';
import { kv } from '../../db/index.ts';
import { CLAUDE_BIN } from './accounts.ts';

/**
 * Claude Code models as the CLI itself lists them (the /model picker): the `initialize`
 * control request of stream-json mode returns `models`. No prompt is sent, so no tokens
 * are spent. Cached for 6 hours (kv); Harness → Refresh re-reads it.
 */

const KEY = 'claude.models';
const TTL_MS = 6 * 3600_000;
const FALLBACK: ModelOption[] = [
  { id: 'opus', label: 'Opus' },
  { id: 'sonnet', label: 'Sonnet' },
  { id: 'haiku', label: 'Haiku' },
];

type Json = Record<string, any>;

function readModels(): Promise<ModelOption[] | null> {
  return new Promise((resolve) => {
    let p: ReturnType<typeof spawn>;
    try {
      p = spawn(CLAUDE_BIN, ['-p', '--input-format', 'stream-json', '--output-format', 'stream-json', '--verbose'], {
        cwd: tmpdir(),
        env: childEnv(),
        stdio: ['pipe', 'pipe', 'ignore'],
      });
    } catch {
      return resolve(null);
    }
    let buf = '';
    let done = false;
    const finish = (v: ModelOption[] | null) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      p.kill();
      resolve(v);
    };
    const timer = setTimeout(() => finish(null), 30_000);
    p.on('error', () => finish(null));
    p.on('close', () => finish(null));
    p.stdout!.on('data', (d: Buffer) => {
      buf += d.toString();
      let nl: number;
      while ((nl = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, nl);
        buf = buf.slice(nl + 1);
        let j: Json;
        try {
          j = JSON.parse(line);
        } catch {
          continue;
        }
        if (j.type !== 'control_response') continue;
        const models = (j.response?.response ?? j.response)?.models;
        if (!Array.isArray(models)) return finish(null);
        finish(
          models
            .filter((m: Json) => typeof m?.value === 'string')
            .map((m: Json) => ({ id: m.value, label: m.displayName || m.value, description: m.description || undefined, resolved: m.resolvedModel || undefined })),
        );
      }
    });
    p.stdin!.write(JSON.stringify({ type: 'control_request', request_id: 'hub-models', request: { subtype: 'initialize' } }) + '\n');
  });
}

let inflight: Promise<ModelOption[]> | null = null;

/** Models offered by the installed Claude Code; the first is "Default (recommended)". */
export function claudeModelList(refresh = false): Promise<ModelOption[]> {
  const saved = kv.get<ModelOption[]>(KEY);
  if (!refresh && saved && Date.now() - Date.parse(saved.updatedAt) < TTL_MS) return Promise.resolve(saved.value);
  inflight ??= readModels()
    .then((list) => {
      if (list?.length) kv.set(KEY, list);
      return list?.length ? list : (saved?.value ?? FALLBACK);
    })
    .finally(() => (inflight = null));
  return inflight;
}
