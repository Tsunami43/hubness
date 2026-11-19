import { createCipheriv, createDecipheriv, createHash, randomBytes } from 'node:crypto';
import { chmodSync, existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { SecretInfo } from '@hubness/shared';
import { config } from './config.ts';
import { secretRows } from './db/index.ts';
import { HttpError } from './errors.ts';

/**
 * Project secrets (like Secrets in Replit): encrypted in the DB with AES-256-GCM,
 * decrypted only on the server when a process starts (Run, Shell, agent)
 * and injected as environment variables. Values never reach the frontend.
 *
 * Keys: HUBNESS_SECRET_KEY — one or more comma-separated strings of ≥ 32
 * characters (SHA-256 is taken). The first key encrypts, the others only
 * decrypt — this is how keys are rotated without losing secrets. Each value
 * records its key id (a hash prefix), so the right key is found immediately.
 * If the variable is not set, a key is generated in <data>/secret.key (mode
 * 600). Keep this file separate from DB backups: without it the secrets cannot
 * be decrypted.
 */

const KEY_FILE = join(config.dataDir, 'secret.key');
type Key = { id: string; key: Buffer };
let cachedKeys: Key[] | null = null;

/** For tests: reload keys (after changing HUBNESS_SECRET_KEY). */
export function resetKeys() {
  cachedKeys = null;
}

export function keySource(): 'env' | 'file' {
  return process.env.HUBNESS_SECRET_KEY ? 'env' : 'file';
}

function keys(): Key[] {
  if (cachedKeys) return cachedKeys;
  let materials = (process.env.HUBNESS_SECRET_KEY ?? '').split(',').map((k) => k.trim()).filter(Boolean);
  if (materials.some((m) => m.length < 32)) throw new HttpError(500, 'Every HUBNESS_SECRET_KEY must be at least 32 characters');
  if (!materials.length) {
    if (!existsSync(KEY_FILE)) writeFileSync(KEY_FILE, randomBytes(32).toString('base64') + '\n', { mode: 0o600 });
    chmodSync(KEY_FILE, 0o600);
    materials = [readFileSync(KEY_FILE, 'utf8').trim()];
  }
  cachedKeys = materials.map((m) => {
    const key = createHash('sha256').update(m).digest();
    return { key, id: createHash('sha256').update('hubness-secret-key:').update(key).digest('hex').slice(0, 8) };
  });
  return cachedKeys;
}

// the value is bound to the project and name (AAD): swapped rows in the DB won't decrypt
const aad = (projectId: string, name: string) => Buffer.from(`${projectId}\0${name}`);

export function encrypt(projectId: string, name: string, value: string): string {
  const k = keys()[0]!;
  const iv = randomBytes(12);
  const c = createCipheriv('aes-256-gcm', k.key, iv);
  c.setAAD(aad(projectId, name));
  const ct = Buffer.concat([c.update(value, 'utf8'), c.final()]);
  return ['v1', k.id, iv.toString('base64'), c.getAuthTag().toString('base64'), ct.toString('base64')].join(':');
}

export function decrypt(projectId: string, name: string, blob: string): string {
  const [v, kid, iv, tag, ct] = blob.split(':');
  if (v !== 'v1' || !kid || !iv || !tag || ct === undefined) throw new Error('Unknown secret format');
  const k = keys().find((x) => x.id === kid);
  if (!k) throw new Error(`Secret key ${kid} is not configured`);
  const d = createDecipheriv('aes-256-gcm', k.key, Buffer.from(iv, 'base64'));
  d.setAAD(aad(projectId, name));
  d.setAuthTag(Buffer.from(tag, 'base64'));
  return Buffer.concat([d.update(Buffer.from(ct, 'base64')), d.final()]).toString('utf8');
}

const NAME_RE = /^[A-Za-z_][A-Za-z0-9_]{0,127}$/;
// variables that break startup or hijack the hub
const RESERVED = new Set(['PATH', 'HOME', 'USER', 'SHELL', 'TERM', 'PWD', 'LD_PRELOAD', 'LD_LIBRARY_PATH', 'NODE_OPTIONS']);

export function listSecrets(projectId: string): SecretInfo[] {
  return secretRows.list(projectId).map((r) => ({ name: r.name, updatedAt: r.updatedAt }));
}

export function setSecret(projectId: string, name: unknown, value: unknown): SecretInfo[] {
  if (typeof name !== 'string' || !NAME_RE.test(name)) throw new HttpError(400, 'Name: letters, digits and _ (like an env variable), not starting with a digit');
  if (RESERVED.has(name.toUpperCase()) || name.startsWith('HUB_') || name.startsWith('HUBNESS_')) throw new HttpError(400, `${name} is reserved`);
  if (typeof value !== 'string' || !value.length) throw new HttpError(400, 'Value is required');
  if (value.length > 64 * 1024) throw new HttpError(400, 'Value is too long (max 64 KB)');
  secretRows.set(projectId, name, encrypt(projectId, name, value));
  return listSecrets(projectId);
}

export function deleteSecret(projectId: string, name: string): SecretInfo[] {
  secretRows.delete(projectId, name);
  return listSecrets(projectId);
}

/** Secrets that can't be decrypted with the current keys (key rotated without the old one). */
export function unreadableSecrets(projectId: string): string[] {
  return secretRows.list(projectId).filter((r) => {
    try {
      decrypt(projectId, r.name, r.value);
      return false;
    } catch {
      return true;
    }
  }).map((r) => r.name);
}

/** Environment from project secrets — only for starting processes on the server. */
export function secretEnv(projectId: string): Record<string, string> {
  const env: Record<string, string> = {};
  for (const r of secretRows.list(projectId)) {
    try {
      env[r.name] = decrypt(projectId, r.name, r.value);
    } catch {
      // key was rotated — the secret is unreadable; don't inject it and don't fail the run
    }
  }
  return env;
}
