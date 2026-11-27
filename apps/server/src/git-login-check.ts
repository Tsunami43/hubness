/**
 * Is a stored git login still good? The hub asks the host's API "who am I" with the login:
 * the answer says whether it is accepted, who it is there, and where the account's picture
 * is — which the hub downloads and keeps, so the browser shows it from the hub (a private
 * host serves the picture only with the login, and the browser asks no other server).
 *
 * Hosts are recognised by name (github.*, gitlab.*, bitbucket.org, codeberg.org); for any
 * other host the known APIs are tried in turn: GitLab, Gitea / Forgejo, GitHub Enterprise.
 * The Rust server does the same in `git_login_check.rs`.
 */
import type { GitCredentialInfo, GitCredentialKind } from '@hubness/shared';
import { gitCredentialRows, type GitCheckResult } from './db/credentials.ts';
import { HttpError } from './errors.ts';
import { listGitCredentials, normalizeHost, secretOf } from './git-auth.ts';

const TIMEOUT_MS = 8000;
const MAX_AVATAR = 512 * 1024;

export interface Login {
  username: string;
  kind: GitCredentialKind;
  secret: string;
}

/** One "who am I" request: where, with which headers, and how to read the answer. */
export interface Probe {
  api: 'github' | 'gitlab' | 'gitea' | 'bitbucket';
  url: string;
  headers: Record<string, string>;
}

const basic = (l: Login) => `Basic ${Buffer.from(`${l.username}:${l.secret}`).toString('base64')}`;

/**
 * Where a host's API lives. HUBNESS_GIT_API (tests) puts every host under one address:
 * `<base>/<api host>/…`.
 */
export function apiRoot(apiHost: string): string {
  const base = process.env.HUBNESS_GIT_API;
  return base ? `${base.replace(/\/+$/, '')}/${apiHost}` : `https://${apiHost}`;
}

/** Which host's API an address belongs to (under HUBNESS_GIT_API: the first part of the path). */
export function apiHostOf(url: URL): string {
  const base = process.env.HUBNESS_GIT_API?.replace(/\/+$/, '');
  if (base && (url.href === base || url.href.startsWith(`${base}/`))) return url.href.slice(base.length + 1).split(/[/?#]/)[0] ?? '';
  return url.host;
}

/** The requests to try for a host, most likely first. */
export function probes(host: string, l: Login): Probe[] {
  const github = (root: string): Probe => ({ api: 'github', url: `${root}/user`, headers: { authorization: `Bearer ${l.secret}`, accept: 'application/vnd.github+json' } });
  // a GitLab token works as a bearer; a password never does (the API takes tokens only)
  const gitlab: Probe = { api: 'gitlab', url: `${apiRoot(host)}/api/v4/user`, headers: { authorization: `Bearer ${l.secret}` } };
  // Gitea / Forgejo take a token, or the name and password
  const gitea: Probe = { api: 'gitea', url: `${apiRoot(host)}/api/v1/user`, headers: { authorization: l.kind === 'token' ? `token ${l.secret}` : basic(l) } };
  if (host === 'github.com') return [github(apiRoot('api.github.com'))];
  if (host.startsWith('github.')) return [github(`${apiRoot(host)}/api/v3`)];
  if (host === 'gitlab.com' || host.startsWith('gitlab.')) return [gitlab];
  if (host === 'bitbucket.org') return [{ api: 'bitbucket', url: `${apiRoot('api.bitbucket.org')}/2.0/user`, headers: { authorization: basic(l) } }];
  if (host === 'codeberg.org' || host.startsWith('gitea.') || host.startsWith('forgejo.')) return [gitea];
  return [gitlab, gitea, github(`${apiRoot(host)}/api/v3`)];
}

/** The account in a "who am I" answer: its name on the host and its picture. */
export function account(api: Probe['api'], body: unknown): { name: string; avatarUrl: string } | null {
  if (!body || typeof body !== 'object') return null;
  const b = body as Record<string, any>;
  const str = (v: unknown) => (typeof v === 'string' ? v : '');
  const name = api === 'gitlab' ? str(b.username) : api === 'bitbucket' ? str(b.username) || str(b.nickname) || str(b.display_name) : str(b.login) || str(b.username);
  if (!name) return null;
  return { name: name.slice(0, 200), avatarUrl: api === 'bitbucket' ? str(b.links?.avatar?.href) : str(b.avatar_url) };
}

async function get(url: string, headers: Record<string, string>): Promise<Response> {
  return fetch(url, { headers: { 'user-agent': 'hubness', ...headers }, redirect: 'follow', signal: AbortSignal.timeout(TIMEOUT_MS) });
}

/** The account's picture: an image of a sane size, else none. The login goes only to the host itself. */
async function avatar(host: string, probe: Probe, raw: string): Promise<{ type: string; data: Uint8Array } | null> {
  let url: URL;
  try {
    // GitLab gives a path for uploaded pictures
    url = new URL(raw, new URL(probe.url).origin);
  } catch {
    return null;
  }
  if (url.protocol !== 'https:' && url.protocol !== 'http:') return null;
  // the login goes with the request only to the host that just accepted it — never to a
  // picture server (avatars.githubusercontent.com, gravatar)
  const own = apiHostOf(url) === apiHostOf(new URL(probe.url)) || apiHostOf(url) === host;
  try {
    const res = await get(url.href, own ? probe.headers : {});
    const type = (res.headers.get('content-type') ?? '').split(';')[0]!.trim().toLowerCase();
    // no SVG: it is a document that can carry scripts, and it would be served from the hub's own address
    if (!res.ok || !/^image\/(png|jpeg|gif|webp|avif)$/.test(type)) return null;
    const data = new Uint8Array(await res.arrayBuffer());
    return data.length && data.length <= MAX_AVATAR ? { type, data } : null;
  } catch {
    return null;
  }
}

/** Ask the host. Never throws: what went wrong is the result. */
export async function checkLogin(host: string, l: Login): Promise<GitCheckResult> {
  let refused = 0;
  let unreachable = '';
  for (const probe of probes(host, l)) {
    let res: Response;
    try {
      res = await get(probe.url, probe.headers);
    } catch (e) {
      unreachable = (e as Error).name === 'TimeoutError' ? 'timed out' : ((e as { cause?: { code?: string } }).cause?.code ?? (e as Error).message);
      continue;
    }
    if (res.status === 401 || res.status === 403) {
      refused = res.status;
      continue;
    }
    if (!res.ok) continue;
    const who = account(probe.api, await res.json().catch(() => null));
    if (!who) continue;
    return { status: 'ok', error: '', account: who.name, avatar: who.avatarUrl ? await avatar(host, probe, who.avatarUrl) : null };
  }
  // a refusal keeps the picture: it is still that account, it only has to sign in again
  if (refused) {
    // the APIs take tokens; a password that git itself accepts can't be checked this way
    if (l.kind === 'password') return { status: 'unknown', error: `${host} doesn’t check passwords through its API: the login is checked by the first fetch or push.`, account: '' };
    return { status: 'failed', error: `${host} refused the login (${refused}): the token is wrong, expired or revoked.`, account: '' };
  }
  if (unreachable) return { status: 'unknown', error: `Can’t reach ${host}: ${unreachable}`, account: '' };
  return { status: 'unknown', error: `${host} has no API hubness knows: the login is checked by the first fetch or push.`, account: '' };
}

/** `POST /git-credentials/:host/check`: check the stored login and remember the answer. */
export async function checkGitCredential(hostInput: string): Promise<GitCredentialInfo[]> {
  const host = normalizeHost(hostInput);
  const row = host ? gitCredentialRows.list().find((r) => r.host === host) : undefined;
  if (!host || !row) throw new HttpError(404, 'No login for this host');
  const secret = secretOf(row);
  if (secret === null) throw new HttpError(409, 'The stored login can’t be decrypted (the secret key changed): save it again');
  const result = await checkLogin(host, { username: row.username, kind: row.kind as GitCredentialKind, secret });
  // the login may have been replaced or removed while the host was thinking
  const now = gitCredentialRows.list().find((r) => r.host === host);
  if (now && now.updatedAt === row.updatedAt) gitCredentialRows.checked(host, result);
  return listGitCredentials();
}

/** `GET /git-credentials/:host/avatar`. */
export function gitCredentialAvatar(hostInput: string): { type: string; data: Uint8Array } {
  const host = normalizeHost(hostInput);
  const a = host ? gitCredentialRows.avatar(host) : null;
  if (!a) throw new HttpError(404, 'No picture');
  return a;
}
