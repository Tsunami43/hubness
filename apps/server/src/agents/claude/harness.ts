import { existsSync, realpathSync, renameSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import type { HarnessAccount, HarnessAction, HarnessAuth, HarnessInfo, LimitWindow } from '@hubness/shared';
import { kv } from '../../db/index.ts';
import { HttpError } from '../../errors.ts';
import { shq } from '../../terminal.ts';
import { action, cachedLatest, npmLatest, parseVersion, readJson, resolveBin, runCli, versionInfo, type Json } from '../kit.ts';
import { CLAUDE_BIN, DEFAULT_CLAUDE_PROFILE, profileClaudeJson, profileEnv, type ClaudeProfile } from './accounts.ts';
import { claudeModelList } from './models.ts';

/**
 * Claude Code harness: version and update, login, subscription limits, default model.
 * Everything is read from official CLI commands and its local files; tokens are not read.
 */

export const KIND = 'claude-code';

// ---------- subscription limits ----------

const limitsKey = (profileId: string) => (profileId === 'default' ? 'claude.limits' : `claude.limits:${profileId}`);

/** Called by the adapter on every rate_limit_event — so limits are always fresh. */
export function recordClaudeLimits(info: Json, profileId = 'default') {
  const windows = (info?.unifiedWindows ?? {}) as Record<string, Json>;
  const limits: LimitWindow[] = Object.entries(windows)
    .filter(([, w]) => typeof w?.utilization === 'number')
    .map(([id, w]) => ({
      id,
      utilization: Math.round(w.utilization <= 1 ? w.utilization * 100 : w.utilization),
      resetsAt: w.resetsAt ? new Date(Number(w.resetsAt) * 1000).toISOString() : null,
    }));
  if (limits.length) kv.set(limitsKey(profileId), limits);
}

/**
 * Limits from the CLI's own cache (.claude.json → cachedUsageUtilization), what /usage shows.
 * `limits` is the generic list (any plan: session, daily, weekly, per-model weekly…);
 * older CLIs only had fixed windows (five_hour, seven_day…).
 */
export function cliCachedLimits(claudeJson: Json | null): { limits: LimitWindow[]; at: string } | null {
  const c = claudeJson?.cachedUsageUtilization;
  const u = c?.utilization;
  if (!u) return null;
  let limits: LimitWindow[] = [];
  if (Array.isArray(u.limits)) {
    limits = (u.limits as Json[])
      .filter((l) => typeof l?.percent === 'number' && typeof l.kind === 'string')
      .map((l) => {
        const scope: string | null = l.scope?.model?.display_name ?? l.scope?.surface?.display_name ?? (typeof l.scope?.surface === 'string' ? l.scope.surface : null);
        return { id: scope ? `${l.kind}:${scope}` : l.kind, group: l.group ?? null, scope, utilization: Math.round(l.percent), resetsAt: l.resets_at ?? null };
      });
  }
  if (!limits.length) {
    const known = ['five_hour', 'seven_day', 'seven_day_opus', 'seven_day_sonnet'];
    limits = known
      .filter((id) => u[id] && typeof u[id].utilization === 'number')
      .map((id) => ({ id, utilization: Math.round(u[id].utilization), resetsAt: u[id].resets_at ?? null }));
  }
  return limits.length ? { limits, at: new Date(c.fetchedAtMs).toISOString() } : null;
}

// our rate_limit_event windows → the CLI limit they update
const WINDOW_OF: Record<string, (l: LimitWindow) => boolean> = {
  seven_day: (l) => l.id === 'seven_day' || l.id === 'weekly_all',
  five_hour: (l) => l.id === 'five_hour' || (l.group === 'session' && !l.scope),
};

/** CLI list (all windows) with fresher numbers from our own turns where they match. */
export function mergeLimits(cli: { limits: LimitWindow[]; at: string } | null, ours: { value: LimitWindow[]; updatedAt: string } | undefined) {
  if (!cli) return { limits: ours?.value ?? [], asOf: ours?.updatedAt ?? null };
  if (!ours || ours.updatedAt <= cli.at) return { limits: cli.limits, asOf: cli.at };
  const limits = cli.limits.map((l) => {
    const w = ours.value.find((o) => WINDOW_OF[o.id]?.(l));
    return w ? { ...l, utilization: w.utilization, resetsAt: w.resetsAt ?? l.resetsAt } : l;
  });
  // a window we saw that the CLI cache doesn't have yet
  for (const o of ours.value) if (!cli.limits.some((l) => WINDOW_OF[o.id]?.(l) ?? l.id === o.id)) limits.push(o);
  return { limits, asOf: ours.updatedAt };
}

/** The headline limit goes first: the UI shows the first one in the stats row. */
const mainFirst = (limits: LimitWindow[]) => {
  const i = limits.findIndex((w) => w.id === 'weekly_all' || w.id === 'seven_day');
  return i <= 0 ? limits : [limits[i]!, ...limits.slice(0, i), ...limits.slice(i + 1)];
};

// ---------- account ----------

function humanTier(tier: string | null | undefined): string | null {
  if (!tier) return null;
  const m = tier.match(/max_(\d+x)/);
  return m ? `Max ${m[1]}` : tier.replace(/^default_/, '').replace(/_/g, ' ');
}

async function claudeAuth(p: ClaudeProfile): Promise<HarnessAuth | null> {
  const statusOut = await runCli(CLAUDE_BIN, ['auth', 'status'], 20_000, profileEnv(p));
  if (!statusOut) return null;
  let st: Json = {};
  try {
    st = JSON.parse(statusOut);
  } catch {}
  const oa = readJson(profileClaudeJson(p))?.oauthAccount ?? {};
  const sub: string | null = st.subscriptionType ?? null;
  return {
    loggedIn: Boolean(st.loggedIn),
    method: st.authMethod && st.authMethod !== 'none' ? st.authMethod : null,
    plan: humanTier(oa.userRateLimitTier ?? oa.organizationRateLimitTier) ?? (sub ? sub[0]!.toUpperCase() + sub.slice(1) : null),
    planDetail: oa.planDisplayName ?? null,
    email: st.email ?? oa.emailAddress ?? null,
    org: st.orgName ?? oa.organizationName ?? null,
    expiresAt: st.loginExpiresAt ?? null,
    extraUsage: typeof oa.hasExtraUsageEnabled === 'boolean' ? oa.hasExtraUsageEnabled : null,
  };
}

/** Cheap check for pickers: the CLI keeps the signed-in account in ~/.claude.json. */
export const claudeSignedIn = () => Boolean(readJson(profileClaudeJson(DEFAULT_CLAUDE_PROFILE))?.oauthAccount) || Boolean(process.env.ANTHROPIC_API_KEY);

// ---------- default model ----------

const claudeSettingsPath = () => join(homedir(), '.claude', 'settings.json');

/** The CLI's default model: `model` in ~/.claude/settings.json (what /model → Enter writes); null — the CLI's recommended one. */
export function claudeDefaultModel(): string | null {
  const m = readJson(claudeSettingsPath())?.model;
  return typeof m === 'string' && m ? m : null;
}

/** Set / clear `model` in ~/.claude/settings.json; the rest of the file is untouched. Written atomically. */
export function setClaudeDefaultModel(model: string | null) {
  if (model !== null && !/^[\w.:\[\]-]{1,100}$/.test(model)) throw new HttpError(400, 'Bad model id');
  const path = claudeSettingsPath();
  let cfg: Json = {};
  if (existsSync(path)) {
    const parsed = readJson(path);
    if (!parsed) throw new HttpError(409, 'settings.json is not plain JSON — edit it by hand');
    cfg = parsed;
  }
  if (model) cfg.model = model;
  else delete cfg.model;
  // through the real path: a symlinked settings.json stays a symlink
  const real = existsSync(path) ? realpathSync(path) : path;
  const tmp = `${real}.hub-${process.pid}.tmp`;
  writeFileSync(tmp, JSON.stringify(cfg, null, 2) + '\n');
  renameSync(tmp, real);
}

// ---------- harness ----------

/** Update command by install method: the binary's npm prefix or its own updater. */
function claudeUpdateCommand(real: string | null): string {
  const m = real?.match(/^(.*)\/lib\/node_modules\/@anthropic-ai\/claude-code\//);
  if (m) return `npm install -g --prefix ${shq(m[1]!)} @anthropic-ai/claude-code@latest`;
  return `${shq(CLAUDE_BIN)} update`;
}

export async function claudeHarness(refresh = false): Promise<HarnessInfo> {
  const active = DEFAULT_CLAUDE_PROFILE;
  const [versionOut, real] = await Promise.all([runCli(CLAUDE_BIN, ['--version']), resolveBin(CLAUDE_BIN)]);
  const installed = parseVersion(versionOut);
  const latest = await cachedLatest(KIND, 'npm', () => npmLatest('@anthropic-ai/claude-code'));
  const npmPrefix = real?.match(/^(.*)\/lib\/node_modules\//)?.[1];
  const update = claudeUpdateCommand(real);
  const version = versionInfo(installed, latest, npmPrefix ? `npm (${npmPrefix})` : 'native installer', update);

  const bin = shq(CLAUDE_BIN);
  const actions: HarnessAction[] = installed
    ? [action('update', update), action('login', `${bin} auth login`), action('login-console', `${bin} auth login --console`), action('logout', `${bin} auth logout`)]
    : [action('install', 'npm install -g @anthropic-ai/claude-code@latest')];

  // one account: ~/.claude
  const auth = installed ? await claudeAuth(active) : null;
  const accounts: HarnessAccount[] = installed
    ? [{ id: active.id, label: 'claude.ai', detail: auth?.email ?? null, loggedIn: Boolean(auth?.loggedIn), active: true }]
    : [];

  // limits: every window the CLI knows (/usage), with fresher numbers from our turns
  const { limits, asOf } = mergeLimits(cliCachedLimits(readJson(profileClaudeJson(active))), kv.get<LimitWindow[]>(limitsKey(active.id)));
  const models = installed ? await claudeModelList(refresh) : [];
  return {
    kind: KIND,
    label: 'Claude Code',
    protocol: 'cli',
    pinned: false,
    version,
    actions,
    accounts,
    auth,
    accountMode: 'single',
    limits: mainFirst(limits),
    limitsAsOf: asOf,
    models,
    defaultModel: claudeDefaultModel(),
    defaultModelEdit: 'options',
    usage: null,
  };
}
