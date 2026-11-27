import { spawnSync } from 'node:child_process';
import { existsSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { GitAuthProblem, GitCredentialInfo, GitCredentialKind } from '@hubness/shared';
import { gitCredentialRows } from './db/index.ts';
import { HttpError } from './errors.ts';
import { parseRemote } from './git-providers.ts';
import { decrypt, encrypt } from './secrets.ts';
import { childEnv, config } from './config.ts';
import { shq as qsh, startJob } from './terminal.ts';

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
  return gitCredentialRows.list().map((r) => ({
    host: r.host,
    username: r.username,
    kind: r.kind as GitCredentialKind,
    updatedAt: r.updatedAt,
    status: r.checkStatus === 'ok' || r.checkStatus === 'failed' ? r.checkStatus : 'unknown',
    checkedAt: r.checkedAt,
    account: r.account,
    error: r.checkError,
    avatar: r.hasAvatar,
  }));
}

/** The decrypted secret of a stored login; null when the secret key no longer opens it. */
export function secretOf(row: { host: string; secret: string }): string | null {
  try {
    return decrypt(NS, row.host, row.secret);
  } catch {
    return null;
  }
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

// ---------- sign-in with the GitHub CLI ----------

/** The gh the hub downloads when the host has none (GitHub's release archives). */
const GH_VERSION = '2.83.1';
/** `<data>/gh`: the downloaded gh (`bin/gh`) and a throwaway config for one sign-in. */
const ghRoot = () => join(config.dataDir, 'gh');

/**
 * The sign-in as a shell script (the same text as `gh_login_script` in `git_auth.rs`):
 * `gh auth login --web` prints a one-time code and the device page, then waits for the
 * approval. The config is a throwaway folder and the token is kept in it (not the system
 * keyring); `ssh` + `--skip-ssh-key` keeps gh away from the git config.
 */
export function ghLoginScript(root: string, host: string): string {
  const bin = `${root}/bin/gh`;
  const dl = `${root}/dl`;
  const base = `https://github.com/cli/cli/releases/download/v${GH_VERSION}/gh_${GH_VERSION}_`;
  return [
    'set -e',
    'fetch() { if command -v curl >/dev/null 2>&1; then curl -fsSL "$1"; else wget -qO- "$1"; fi; }',
    'if command -v gh >/dev/null 2>&1; then GH=gh; else',
    `  GH=${qsh(bin)}`,
    '  if [ ! -x "$GH" ]; then',
    '    case "$(uname -s)-$(uname -m)" in',
    '      Linux-x86_64) A=linux_amd64 ;; Linux-aarch64|Linux-arm64) A=linux_arm64 ;; Darwin-arm64) A=macOS_arm64 ;; Darwin-x86_64) A=macOS_amd64 ;;',
    "      *) echo 'No GitHub CLI for this platform: install gh'; exit 1 ;;",
    '    esac',
    `    echo "Downloading the GitHub CLI ${GH_VERSION}…"`,
    `    rm -rf ${qsh(dl)} && mkdir -p ${qsh(dl)} ${qsh(`${root}/bin`)}`,
    '    case "$A" in',
    `      macOS*) fetch "${base}$A.zip" > ${qsh(`${dl}/gh.zip`)} && (cd ${qsh(dl)} && unzip -q gh.zip) ;;`,
    `      *) fetch "${base}$A.tar.gz" | tar -xz -C ${qsh(dl)} ;;`,
    '    esac',
    `    cp ${qsh(dl)}/gh_${GH_VERSION}_"$A"/bin/gh "$GH"`,
    `    rm -rf ${qsh(dl)}`,
    '  fi',
    'fi',
    `export GH_CONFIG_DIR=${qsh(`${root}/config`)}`,
    'rm -rf "$GH_CONFIG_DIR"',
    `BROWSER=true GH_BROWSER=true "$GH" auth login --hostname ${qsh(host)} --git-protocol ssh --skip-ssh-key --web --scopes workflow --insecure-storage </dev/null`,
  ].join('\n');
}

/** After a successful sign-in: the token and the account name from gh, saved as the host's login. */
function ghCollect(host: string) {
  const cfg = join(ghRoot(), 'config');
  const own = join(ghRoot(), 'bin', 'gh');
  const gh = existsSync(own) ? own : 'gh';
  const run = (args: string[]) => {
    const r = spawnSync(gh, args, { env: { ...childEnv(), GH_CONFIG_DIR: cfg }, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
    const out = (r.stdout ?? '').trim();
    return r.status === 0 && out ? out : null;
  };
  const token = run(['auth', 'token', '--hostname', host]);
  const login = run(['api', '--hostname', host, 'user', '--jq', '.login']);
  rmSync(cfg, { recursive: true, force: true });
  if (!token) throw new Error('gh signed in but gave no token');
  setGitCredential(host, { username: login ?? 'git', kind: 'token', secret: token });
}

/** Sign in to a GitHub host with the GitHub CLI (a terminal job); on success the token becomes the host's login. */
export function ghLogin(hostInput: string, size: { cols?: number; rows?: number }) {
  const host = normalizeHost(hostInput);
  if (!host) throw new HttpError(400, 'host: a domain like github.com');
  return startJob({
    title: `gh · ${host}`,
    command: ghLoginScript(ghRoot(), host),
    display: `gh auth login --hostname ${host} --web`,
    cols: size.cols,
    rows: size.rows,
    onExit: (code) => {
      if (code !== 0) return rmSync(join(ghRoot(), 'config'), { recursive: true, force: true });
      try {
        ghCollect(host);
      } catch (e) {
        console.error(`hubness: gh sign-in for ${host}: ${(e as Error).message}`);
      }
    },
  });
}
