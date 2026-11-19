import type { GitProviderId } from '@hubness/shared';

/**
 * Git providers: detecting the hosting from the remote URL, and web links.
 * New provider = new adapter in PROVIDERS (no changes to the rest of the code).
 * Provider APIs (PRs, repository lists) will come together with the secrets
 * store (docs/PLATFORM.md, phase 4).
 */

export interface ParsedRemote {
  host: string;
  /** owner/repo (GitLab may have group/subgroup/repo) */
  path: string;
}

export interface GitProviderAdapter {
  id: GitProviderId;
  label: string;
  match(remote: ParsedRemote): boolean;
  links(remote: ParsedRemote): { repo: string; branch(b: string): string; commit(sha: string): string; newPullRequest(head: string, base: string | null): string };
}

/** git@host:owner/repo.git, ssh://git@host[:port]/owner/repo.git, https://[user@]host/owner/repo(.git) → host + path. */
export function parseRemote(url: string): ParsedRemote | null {
  const u = url.trim();
  let m = u.match(/^[\w.-]+@([\w.-]+):(?!\/)(.+?)(?:\.git)?\/?$/);
  if (m) return { host: m[1].toLowerCase(), path: m[2] };
  m = u.match(/^(?:ssh|https?|git):\/\/(?:[^@/]+@)?([\w.-]+)(?::\d+)?\/(.+?)(?:\.git)?\/?$/);
  if (m) return { host: m[1].toLowerCase(), path: m[2] };
  return null;
}

/** URL without credentials (https://user:token@host/... → https://host/...). */
export function stripCredentials(url: string): string {
  return url.replace(/^(https?:\/\/)[^@/]+@/, '$1');
}

const enc = (s: string) => s.split('/').map(encodeURIComponent).join('/');

const PROVIDERS: GitProviderAdapter[] = [
  {
    id: 'github',
    label: 'GitHub',
    match: (r) => r.host === 'github.com' || r.host.startsWith('github.'),
    links: (r) => {
      const base = `https://${r.host}/${r.path}`;
      return {
        repo: base,
        branch: (b) => `${base}/tree/${enc(b)}`,
        commit: (sha) => `${base}/commit/${sha}`,
        newPullRequest: (head, b) => `${base}/compare/${b ? `${enc(b)}...` : ''}${enc(head)}?expand=1`,
      };
    },
  },
  {
    id: 'gitlab',
    label: 'GitLab',
    match: (r) => r.host === 'gitlab.com' || r.host.startsWith('gitlab.'),
    links: (r) => {
      const base = `https://${r.host}/${r.path}`;
      return {
        repo: base,
        branch: (b) => `${base}/-/tree/${enc(b)}`,
        commit: (sha) => `${base}/-/commit/${sha}`,
        newPullRequest: (head, b) =>
          `${base}/-/merge_requests/new?merge_request%5Bsource_branch%5D=${encodeURIComponent(head)}${b ? `&merge_request%5Btarget_branch%5D=${encodeURIComponent(b)}` : ''}`,
      };
    },
  },
  {
    id: 'bitbucket',
    label: 'Bitbucket',
    match: (r) => r.host === 'bitbucket.org' || r.host.startsWith('bitbucket.'),
    links: (r) => {
      const base = `https://${r.host}/${r.path}`;
      return {
        repo: base,
        branch: (b) => `${base}/src/${enc(b)}`,
        commit: (sha) => `${base}/commits/${sha}`,
        newPullRequest: (head, b) => `${base}/pull-requests/new?source=${encodeURIComponent(head)}${b ? `&dest=${encodeURIComponent(b)}` : ''}`,
      };
    },
  },
];

export interface ResolvedProvider {
  id: GitProviderId;
  label: string;
  adapter: GitProviderAdapter | null;
  remote: ParsedRemote | null;
}

/** Provider by remote URL; unknown host — generic (no web links). */
export function resolveProvider(url: string | null): ResolvedProvider | null {
  if (!url) return null;
  const remote = parseRemote(url);
  const adapter = remote ? (PROVIDERS.find((p) => p.match(remote)) ?? null) : null;
  return adapter ? { id: adapter.id, label: adapter.label, adapter, remote } : { id: 'generic', label: 'Git', adapter: null, remote };
}

/** Supported URL for git clone: https/ssh/git@host:path. Local paths and git options are not accepted. */
export function isCloneUrl(url: string): boolean {
  if (/\s/.test(url) || url.startsWith('-')) return false;
  return parseRemote(url) !== null;
}

/** Folder name from URL: .../auth-service.git → auth-service. */
export function repoNameFromUrl(url: string): string | null {
  const r = parseRemote(url);
  const last = r?.path.split('/').pop();
  return last ? last.replace(/\.git$/, '') : null;
}
