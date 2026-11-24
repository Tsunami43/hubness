import { fileURLToPath } from 'node:url';
import type { GitAuthProblem, GitCredentialInfo, GitCredentialKind } from '@hubness/shared';
import { gitCredentialRows } from './db/index.ts';
import { HttpError } from './errors.ts';
import { parseRemote } from './git-providers.ts';
import { decrypt, encrypt } from './secrets.ts';

/**
 * Git logins per host (token or login + password over https) for the hub's own git
 * runs: fetch / pull / push from the Git panel and clone. The secret is encrypted like
 * project secrets (secrets.ts) and is handed to git through our credential helper
 * (git-credential-helper.ts) via the environment of that one git process. Agents never
 * get it: their processes are started elsewhere and the sandbox hides git credentials.
 */

// AAD namespace: a row copied to another host won't decrypt
const NS = 'git-credential';
const KINDS: GitCredentialKind[] = ['token', 'password'];
const HOST_RE = /^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*$/;

/** "https://user@GitLab.example.com:8443/x" or "gitlab.example.com" → "gitlab.example.com". */
export function normalizeHost(input: string): string | null {
  let h = input.trim().toLowerCase();
  h = h.replace(/^[a-z]+:\/\//, '').replace(/^[^@/]*@/, '');
  h = h.split('/')[0]!.replace(/:\d+$/, '');
  return HOST_RE.test(h) && h.length <= 253 ? h : null;
}

export function listGitCredentials(): GitCredentialInfo[] {
  return gitCredentialRows.list().map((r) => ({ host: r.host, username: r.username, kind: r.kind as GitCredentialKind, updatedAt: r.updatedAt }));
}

// the credential protocol is line-based: no newlines or NUL inside values
const clean = (s: string) => !/[\r\n\0]/.test(s);

export function setGitCredential(hostInput: string, b: { username?: unknown; kind?: unknown; secret?: unknown }): GitCredentialInfo[] {
  const host = normalizeHost(hostInput);
  if (!host) throw new HttpError(400, 'Host: a domain name like gitlab.example.com');
  const kind = b.kind as GitCredentialKind;
  if (!KINDS.includes(kind)) throw new HttpError(400, 'kind must be token or password');
  const username = typeof b.username === 'string' ? b.username.trim() : '';
  if (!username || username.length > 256 || !clean(username)) throw new HttpError(400, 'Username is required');
  const secret = typeof b.secret === 'string' ? b.secret : '';
  if (!secret || secret.length > 4096 || !clean(secret)) throw new HttpError(400, kind === 'token' ? 'Token is required' : 'Password is required');
  gitCredentialRows.set(host, username, kind, encrypt(NS, host, secret));
  return listGitCredentials();
}

export function deleteGitCredential(hostInput: string): GitCredentialInfo[] {
  const host = normalizeHost(hostInput);
  if (host) gitCredentialRows.delete(host);
  return listGitCredentials();
}

/** Decrypted logins for these hosts (rows that can't be decrypted are skipped). */
function credentialsFor(hosts: string[]): Record<string, { username: string; password: string }> {
  const want = new Set(hosts);
  const out: Record<string, { username: string; password: string }> = {};
  for (const r of gitCredentialRows.list()) {
    if (!want.has(r.host)) continue;
    try {
      out[r.host] = { username: r.username, password: decrypt(NS, r.host, r.secret) };
    } catch {
      // secret key rotated without the old one: behave as if nothing is stored
    }
  }
  return out;
}

const HELPER = fileURLToPath(new URL('./git-credential-helper.ts', import.meta.url));
const shq = (s: string) => `'${s.replace(/'/g, `'\\''`)}'`;

/**
 * `-c` options and environment that make git use the stored logins for these remote
 * URLs. The first empty `credential.helper` clears helpers from every config level
 * (including the repository's own), so the login isn't also passed to them on
 * `store`. Nothing is returned when no login is stored for these hosts, and git
 * keeps the user's own helpers.
 */
export function gitAuthFor(urls: string[]): { args: string[]; env: Record<string, string> } {
  const hosts = urls
    .filter((u) => /^https?:\/\//i.test(u.trim()))
    .map((u) => parseRemote(u)?.host)
    .filter((h): h is string => Boolean(h));
  const creds = credentialsFor(hosts);
  if (!Object.keys(creds).length) return { args: [], env: {} };
  return {
    args: ['-c', 'credential.helper=', '-c', `credential.helper=!${shq(process.execPath)} ${shq(HELPER)}`],
    env: { HUB_GIT_CREDENTIALS: JSON.stringify(creds) },
  };
}

/** Recognise "access refused" in git's output of a network operation. */
export function authProblem(output: string, remoteHosts: string[]): GitAuthProblem | null {
  const hostOf = (s: string) => normalizeHost(s) ?? s;
  let m = output.match(/could not read (?:Username|Password) for '(https?):\/\/([^'/]+)/i);
  if (m) return { host: hostOf(m[2]!), protocol: 'https', reason: 'missing' };
  m = output.match(/Authentication failed for '(https?):\/\/([^'/]+)/i);
  if (m) return { host: hostOf(m[2]!), protocol: 'https', reason: 'rejected' };
  m = output.match(/unable to access '(https?):\/\/([^'/]+)[^']*': The requested URL returned error: (401|403)/i);
  if (m) return { host: hostOf(m[2]!), protocol: 'https', reason: 'rejected' };
  m = output.match(/(?:[\w.-]+@)?([\w.-]+): Permission denied \(publickey/i);
  if (m) return { host: hostOf(m[1]!), protocol: 'ssh', reason: 'missing' };
  if (/Host key verification failed/i.test(output)) {
    m = output.match(/host key is known for ([\w.-]+)/i);
    const host = m ? m[1]! : remoteHosts.length === 1 ? remoteHosts[0]! : '';
    return { host, protocol: 'ssh', reason: 'hostkey' };
  }
  return null;
}
