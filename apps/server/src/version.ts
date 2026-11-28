import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { HubVersion } from '@hubness/shared';
import { repoRoot } from './config.ts';

/**
 * The hub's version and the latest release, for the System page. Releases are the GitHub
 * Releases `hubness update` installs from: `<base>/latest/download/VERSION` holds the latest
 * version (`HUBNESS_RELEASE_URL` replaces the base, also with a `file://` folder for tests;
 * `HUBNESS_NO_UPDATE_CHECK=1` turns the check off).
 *
 * The TypeScript server runs from a checkout and can't replace itself: `canUpdate` is false.
 */

export const HUB_VERSION: string = JSON.parse(readFileSync(join(repoRoot, 'apps/server/package.json'), 'utf8')).version;

const RELEASES = 'https://github.com/Tsunami43/hubness/releases';
const CHECK_EVERY_MS = 6 * 60 * 60 * 1000;
/** A failed check is repeated sooner than a successful one. */
const RETRY_MS = 10 * 60 * 1000;

const releasesUrl = () => (process.env.HUBNESS_RELEASE_URL || RELEASES).replace(/\/+$/, '');
const checkDisabled = () => {
  const v = process.env.HUBNESS_NO_UPDATE_CHECK;
  return !!v && v !== '0';
};

type Semver = { core: [number, number, number]; pre: string[] };

export function parseVersion(s: string): Semver | null {
  const m = /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+.*)?$/.exec(s.trim());
  if (!m) return null;
  return { core: [Number(m[1]), Number(m[2]), Number(m[3])], pre: m[4] ? m[4].split('.') : [] };
}

/** `a` against `b` like semver (-1, 0, 1); an unparsable version counts as the oldest. */
export function compareVersions(a: string, b: string): number {
  const x = parseVersion(a);
  const y = parseVersion(b);
  if (!x || !y) return x ? 1 : y ? -1 : 0;
  for (let i = 0; i < 3; i++) if (x.core[i] !== y.core[i]) return x.core[i] > y.core[i] ? 1 : -1;
  if (!x.pre.length || !y.pre.length) return x.pre.length === y.pre.length ? 0 : x.pre.length ? -1 : 1;
  for (let i = 0; i < Math.min(x.pre.length, y.pre.length); i++) {
    const p = x.pre[i];
    const q = y.pre[i];
    if (p === q) continue;
    const pn = /^\d+$/.test(p);
    const qn = /^\d+$/.test(q);
    if (pn && qn) return Number(p) > Number(q) ? 1 : -1;
    if (pn !== qn) return pn ? -1 : 1;
    return p > q ? 1 : -1;
  }
  return Math.sign(x.pre.length - y.pre.length);
}

let cache: { at: number; base: string; latest: string | null } | null = null;
let pending: Promise<string | null> | null = null;

async function fetchLatest(base: string): Promise<string | null> {
  const url = `${base}/latest/download/VERSION`;
  try {
    const body = url.startsWith('file://')
      ? readFileSync(fileURLToPath(url), 'utf8')
      : await fetch(url, { signal: AbortSignal.timeout(5000), headers: { 'User-Agent': `hubness/${HUB_VERSION}` } }).then((r) => {
          if (!r.ok) throw new Error(`HTTP ${r.status}`);
          return r.text();
        });
    const v = body.trim().replace(/^v/, '');
    return parseVersion(v) ? v : null;
  } catch {
    return null;
  }
}

/** The latest released version (cached for hours; `force` checks again). */
export async function latestVersion(force = false): Promise<string | null> {
  if (checkDisabled()) return null;
  const base = releasesUrl();
  const now = Date.now();
  if (!force && cache && cache.base === base && now - cache.at < (cache.latest ? CHECK_EVERY_MS : RETRY_MS)) return cache.latest;
  pending ??= fetchLatest(base).then((latest) => {
    // offline: keep the last known version
    cache = { at: Date.now(), base, latest: latest ?? (cache?.base === base ? cache.latest : null) };
    pending = null;
    return cache.latest;
  });
  return pending;
}

export async function hubVersion(force = false): Promise<HubVersion> {
  const latest = await latestVersion(force);
  const base = releasesUrl();
  return {
    version: HUB_VERSION,
    latest,
    updateAvailable: latest !== null && compareVersions(latest, HUB_VERSION) > 0,
    releaseUrl: latest && /^https?:\/\//.test(base) ? `${base}/tag/v${latest}` : null,
    canUpdate: false,
    update: { state: 'idle', message: null },
  };
}
