import { copyText } from '../lib/clipboard';
import { useCallback, useRef, useState, type ReactNode } from 'react';
import { Navigate, useOutletContext, useParams } from 'react-router';
import {
  ArrowUpCircle,
  Check,
  CircleHelp,
  CircleX,
  Copy,
  LogIn,
  LogOut,
  Play,
  Plus,
  RefreshCw,
  Repeat,
  ShieldAlert,
} from 'lucide-react';
import type { AgentKind, HarnessAccount, HarnessAction, HarnessCommon, AgentInfo, HarnessInfo, HarnessVersion, JobInfo, LimitWindow, ModelOption } from '@hubness/shared';
import { api } from '../lib/api';
import { reloadAgents, useAgents } from '../lib/agents';
import { timeAgo, timeUntil } from '../lib/format';
import { useApi, queryClient } from '../lib/query';
import { AgentIcon, ProviderIcon } from '../components/AgentIcon';
import { Button, ErrorLine, Layer, Spinner } from '../components/ui';
import { SubmenuIndex, SubmenuLayout, type SubmenuItem } from '../components/SubmenuLayout';
import { sheetTermSize, TerminalSheet } from '../components/Terminal';
import { ModelPicker } from '../components/ModelPicker';
import { EXTENSION_PAGES, ExtensionsSummary } from '../components/HarnessExtensions';
import { t, tOptional } from '../i18n';
import type { MessageKey } from '../i18n/en';

type Run = (kind: AgentKind, action: HarnessAction['id']) => Promise<void>;

interface HarnessCtx {
  run: Run;
  /** Re-read the harness data and the agent list (after an action). */
  reload: () => Promise<void>;
}

const useHarness = () => useOutletContext<HarnessCtx>();

/** Search words for every agent tab (the agent name and kind are added per item). */
const KEYWORDS = [
  'version', 'update', 'install', 'login', 'sign in', 'logout', 'account', 'plan', 'subscription', 'limits', 'usage',
  'providers', 'models', 'default model', 'stats', 'tokens', 'acp', 'skills', 'rules', 'agents', 'commands', 'plugins',
  'версия', 'обновление', 'вход', 'аккаунт', 'подписка', 'лимиты', 'провайдеры', 'модели', 'навыки', 'правила', 'агенты', 'команды', 'плагины',
];

const EXT_KEYWORDS: Record<(typeof EXTENSION_PAGES)[number]['to'], string[]> = {
  mcp: ['mcp', 'servers', 'tools', 'catalog', 'registry', 'серверы', 'инструменты', 'каталог'],
  skills: ['skills', 'skill.md', 'import', 'навыки', 'скиллы', 'импорт'],
  rules: ['rules', 'claude.md', 'agents.md', 'memory', 'правила', 'память'],
  subagents: ['subagents', 'agents', 'субагенты', 'агенты'],
  commands: ['commands', 'slash', 'prompts', 'команды', 'слеш'],
  plugins: ['plugins', 'marketplace', 'плагины'],
};

/**
 * Harness: agent submenu with search. Like System, nothing heavy is read up front: the menu
 * uses the light agent list (versions, signed in); a tab reads its own harness when opened.
 */
export function HarnessLayout() {
  const agents = useAgents()?.agents;
  const [job, setJob] = useState<JobInfo | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const reload = useCallback(async () => {
    await Promise.all([queryClient.invalidateQueries({ queryKey: ['harness'] }), reloadAgents()]);
  }, []);

  const jobAction = useRef<HarnessAction['id'] | null>(null);
  const run: Run = async (kind, action) => {
    setNotice(null);
    try {
      const r = await api.harnessAction(kind, action, sheetTermSize(24));
      if ('id' in r) {
        setJob(r);
        jobAction.current = action;
      } else {
        setNotice(r.output);
        await reload();
      }
    } catch (e) {
      setNotice(e instanceof Error ? e.message : String(e));
    }
  };

  // the cross-agent Extensions pages first, then the agents in the server's (registry) order
  const extItems: SubmenuItem[] = EXTENSION_PAGES.map(({ to, icon: Icon, label, hint }) => ({
    to,
    label: t(label),
    hint: t(hint),
    icon: <Icon className="size-4" />,
    group: t('ext.group'),
    keywords: EXT_KEYWORDS[to],
  }));
  const items: SubmenuItem[] = [...extItems, ...(agents ?? []).map((a) => ({
    group: t('harness.agentsGroup'),
    to: a.kind,
    label: a.label,
    hint: a.version ? `v${a.version}` : undefined,
    icon: <AgentIcon kind={a.kind} />,
    trailing: <MenuMark a={a} />,
    keywords: [a.kind, ...KEYWORDS],
  }))];

  return (
    <>
      <SubmenuLayout
        base="/harness"
        title={t('nav.harness')}
        items={items}
        searchPlaceholder={t('harness.search')}
        outletContext={{ run, reload } satisfies HarnessCtx}
      />
      {notice ? (
        <Layer>
          <div className="ui-solid fixed inset-x-4 bottom-20 z-40 mx-auto max-w-lg rounded-xl border border-line bg-surface px-4 py-3 text-sm shadow-lg lg:bottom-6">
            <div className="flex items-start gap-3">
              <span className="min-w-0 flex-1 break-words whitespace-pre-wrap">{notice}</span>
              <button type="button" className="text-muted hover:text-fg" onClick={() => setNotice(null)}>
                {t('common.close')}
              </button>
            </div>
          </div>
        </Layer>
      ) : null}
      <TerminalSheet
        job={job}
        onClose={() => {
          // a login nobody finishes would hang forever: closing the window stops it (installs keep running)
          if (job && jobAction.current?.startsWith('log')) void api.jobKill(job.id).catch(() => {});
          setJob(null);
        }}
        onDone={() => void reload()}
      />
    </>
  );
}

export function HarnessIndex() {
  const first = useAgents()?.agents[0];
  return first ? <SubmenuIndex to={first.kind} /> : null;
}

/** Page of a single agent: reads only this harness, when the tab is opened. */
export function HarnessDetail() {
  const { kind = '' } = useParams();
  const agents = useAgents();
  if (agents && !agents.agents.some((a) => a.kind === kind)) return <Navigate to="/harness" replace />;
  return <HarnessTab key={kind} kind={kind} />;
}

function HarnessTab({ kind }: { kind: AgentKind }) {
  const [refreshing, setRefreshing] = useState(false);
  const { data, setData, loading, error } = useApi(['harness', kind], () => api.harnessOne(kind));
  const refresh = async () => {
    setRefreshing(true);
    try {
      setData(await api.harnessOne(kind, true));
    } finally {
      setRefreshing(false);
    }
  };
  if (error) return <ErrorLine error={error} />;
  if (loading && !data)
    return (
      <div className="flex justify-center py-10">
        <Spinner />
      </div>
    );
  if (!data) return null;
  const info = data.info;
  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2 text-xs text-muted">
        <span className="min-w-0 flex-1 truncate">{t('harness.checked', { time: timeAgo(data.checkedAt) })}</span>
        <Button size="sm" variant="ghost" loading={refreshing} onClick={refresh} title={t('harness.refreshHint')}>
          {!refreshing ? <RefreshCw className="size-3.5" /> : null}
          {t('harness.refresh')}
        </Button>
      </div>
      <AgentCard info={info} />
      <ExtensionsSummary kind={kind} />
    </div>
  );
}

/** Menu status from the light agent list: not installed / not signed in / OK. */
function MenuMark({ a }: { a: AgentInfo }) {
  if (!a.version && !a.available) return <CircleX className="size-4 shrink-0 text-danger" aria-label={t('harness.notInstalled')} />;
  if (a.signedIn === false) return <LogIn className="size-4 shrink-0 text-warn" aria-label={t('harness.notSignedInShort')} />;
  return <Check className="size-4 shrink-0 text-ok" aria-label={t('harness.installed')} />;
}

// ---------- shared card parts ----------

function HarnessCard({
  kind,
  title,
  info,
  stats,
  children,
}: {
  kind: AgentKind;
  title: string;
  info: HarnessCommon;
  stats?: ReactNode;
  children?: ReactNode;
}) {
  const { run } = useHarness();
  const version = info.version;
  const installed = version.status !== 'not-installed';
  const update = info.actions.find((a) => a.id === 'update');
  const installs = info.actions.filter((a) => a.id === 'install');
  return (
    <section className="overflow-hidden rounded-2xl border border-line bg-surface">
      <header className="flex items-center gap-3.5 px-4 py-4 sm:px-5">
        <span className="flex size-11 shrink-0 items-center justify-center rounded-xl border border-line/70 bg-bg">
          <AgentIcon kind={kind} className="size-6" />
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-baseline gap-x-2">
            <h2 className="text-[17px] font-semibold">{title}</h2>
            {version.installed ? <span className="font-mono text-xs text-muted">v{version.installed}</span> : null}
          </div>
          <div className="text-xs text-muted sm:truncate">
            {installed ? t('harness.source', { source: version.source }) : t('harness.notInstalled')}
          </div>
          {/* phone: the badge goes under the title instead of squeezing it */}
          <div className="mt-1.5 sm:hidden">
            <VersionBadge v={version} />
          </div>
        </div>
        <div className="hidden sm:block">
          <VersionBadge v={version} />
        </div>
      </header>

      {!installed && installs.length ? (
        <div className="space-y-2 border-t border-line/60 px-4 py-4 sm:px-5">
          <div className="text-sm">{t('harness.installHint')}</div>
          {installs.map((a) => (
            <CommandLine key={a.command} action={a} onRun={() => run(kind, 'install')} runLabel={t('harness.install')} />
          ))}
        </div>
      ) : null}

      {installed && version.status === 'update-available' && update ? (
        <div className="border-t border-line/60 bg-warn/8 px-4 py-3 sm:px-5">
          <div className="mb-2 text-sm">{t('harness.updateAvailable', { version: version.latest ?? '' })}</div>
          <CommandLine action={update} onRun={() => run(kind, 'update')} runLabel={t('harness.updateNow')} />
        </div>
      ) : null}

      {installed && stats ? <div className="grid border-t border-line/60 sm:grid-cols-3 sm:divide-x sm:divide-line/60">{stats}</div> : null}
      {installed && children ? <div className="space-y-5 border-t border-line/60 px-4 py-4 sm:px-5">{children}</div> : null}
    </section>
  );
}

function VersionBadge({ v }: { v: HarnessVersion }) {
  const map = {
    'up-to-date': { cls: 'text-ok border-ok/30', icon: Check, text: t('harness.upToDate') },
    'update-available': { cls: 'text-warn border-warn/40', icon: ArrowUpCircle, text: t('harness.updateShort') },
    unknown: { cls: 'text-muted border-line', icon: CircleHelp, text: t('harness.unknownShort') },
    'not-installed': { cls: 'text-danger border-danger/40', icon: CircleX, text: t('harness.notInstalled') },
  }[v.status];
  const Icon = map.icon;
  return (
    <span
      title={v.latest ? t('harness.latest', { version: v.latest }) : undefined}
      className={`inline-flex shrink-0 items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs font-medium ${map.cls}`}
    >
      <Icon className="size-3.5" />
      {map.text}
    </span>
  );
}

/** A cell of the stats row under the card header. */
function Stat({ label, value, sub, children }: { label: string; value?: ReactNode; sub?: ReactNode; children?: ReactNode }) {
  return (
    <div className="min-w-0 border-b border-line/60 px-4 py-3.5 last:border-b-0 sm:border-b-0 sm:px-5">
      <div className="text-xs text-muted">{label}</div>
      {value !== undefined ? <div className="mt-1 truncate text-[15px] font-semibold">{value}</div> : null}
      {children}
      {sub ? <div className="mt-0.5 truncate text-xs text-muted">{sub}</div> : null}
    </div>
  );
}

/** Command: copy or run (in the built-in terminal). Root commands can only be copied. */
function CommandLine({ action, onRun, runLabel }: { action: Pick<HarnessAction, 'command' | 'runnable'>; onRun?: () => void; runLabel?: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="flex flex-wrap items-center gap-2 rounded-lg border border-line bg-bg px-3 py-2">
      <span className="font-mono text-xs text-muted select-none">$</span>
      <code className="min-w-0 flex-1 font-mono text-[13px] break-all">{action.command}</code>
      <div className="ml-auto flex shrink-0 items-center gap-1">
        <button
          type="button"
          onClick={() => {
            void copyText(action.command).then((ok) => {
              if (!ok) return;
              setCopied(true);
              setTimeout(() => setCopied(false), 1500);
            });
          }}
          className="inline-flex items-center gap-1 rounded-md px-2 py-1 text-xs text-muted hover:bg-surface hover:text-fg"
        >
          {copied ? <Check className="size-3.5 text-ok" /> : <Copy className="size-3.5" />}
          {copied ? t('harness.copied') : t('harness.copy')}
        </button>
        {onRun && action.runnable ? (
          <button type="button" onClick={onRun} className="btn-accent inline-flex items-center gap-1 rounded-md px-2.5 py-1 text-xs font-medium">
            <Play className="size-3.5" /> {runLabel ?? t('harness.run')}
          </button>
        ) : null}
        {!action.runnable ? (
          <span className="inline-flex items-center gap-1 rounded-md border border-warn/40 px-2 py-1 text-xs text-warn" title={t('harness.rootHint')}>
            <ShieldAlert className="size-3.5" /> {t('harness.root')}
          </span>
        ) : null}
      </div>
    </div>
  );
}

function Section({ title, aside, children }: { title: string; aside?: ReactNode; children: ReactNode }) {
  return (
    <div>
      <div className="mb-2 flex flex-wrap items-baseline gap-x-2">
        <h3 className="text-sm font-semibold">{title}</h3>
        {aside ? <span className="text-xs text-muted">{aside}</span> : null}
      </div>
      {children}
    </div>
  );
}

function Rows({ items }: { items: [string, ReactNode][] }) {
  return (
    <dl className="divide-y divide-line/50 rounded-xl border border-line/60">
      {items.map(([k, v]) => (
        <div key={k} className="flex items-baseline gap-3 px-3.5 py-2.5 text-sm">
          <dt className="w-28 shrink-0 text-muted sm:w-40">{k}</dt>
          <dd className="min-w-0 flex-1 break-words">{v}</dd>
        </div>
      ))}
    </dl>
  );
}

/** Small button in an account row. */
function RowButton({ onClick, children, danger, title }: { onClick: () => void; children: ReactNode; danger?: boolean; title?: string }) {
  return (
    <button
      type="button"
      title={title}
      onClick={onClick}
      className={`inline-flex items-center gap-1 rounded-md border px-2 py-1 text-xs ${
        danger ? 'border-danger/40 text-danger hover:bg-danger/10' : 'border-line text-muted hover:bg-sunken hover:text-fg'
      }`}
    >
      {children}
    </button>
  );
}

/** Dangerous action confirmed within the same button (no system dialogs). */
function ConfirmButton({ onConfirm, label, confirmLabel, icon }: { onConfirm: () => void; label: string; confirmLabel: string; icon: ReactNode }) {
  const [armed, setArmed] = useState(false);
  return armed ? (
    <span className="inline-flex items-center gap-1">
      <RowButton
        danger
        onClick={() => {
          setArmed(false);
          onConfirm();
        }}
      >
        {confirmLabel}
      </RowButton>
      <RowButton onClick={() => setArmed(false)}>{t('common.cancel')}</RowButton>
    </span>
  ) : (
    <RowButton onClick={() => setArmed(true)} title={label}>
      {icon}
      <span className="hidden sm:inline">{label}</span>
    </RowButton>
  );
}

function AccountRow({ a, icon, children }: { a: HarnessAccount; icon?: ReactNode; children?: ReactNode }) {
  return (
    <li className="flex flex-wrap items-center gap-x-3 gap-y-2 px-3.5 py-2.5 text-sm">
      <span className="flex size-8 shrink-0 items-center justify-center rounded-full border border-line bg-bg text-xs font-semibold uppercase">
        {icon ?? a.label.slice(0, 1)}
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="font-medium">{a.label}</span>
          {a.active && a.loggedIn ? <span className="rounded-full bg-ok/15 px-1.5 py-px text-[11px] font-medium text-ok">{t('harness.active')}</span> : null}
          {!a.loggedIn ? <span className="rounded-full bg-warn/15 px-1.5 py-px text-[11px] font-medium text-warn">{t('harness.signedOut')}</span> : null}
        </div>
        {a.detail ? <div className="truncate text-xs text-muted">{a.detail}</div> : null}
      </div>
      <div className="ml-auto flex shrink-0 flex-wrap items-center gap-1">{children}</div>
    </li>
  );
}

/** «Weekly · all models», «Weekly · Fable», «Session (5 h)», «Daily»… as /usage names them. */
const limitLabel = (w: LimitWindow) => {
  if (w.group) {
    const key = `limit.group.${w.group}` as MessageKey;
    const base = t(key) === key ? w.group : t(key);
    return w.scope ? `${base} · ${w.scope}` : w.group === 'weekly' ? `${base} · ${t('limit.allModels')}` : base;
  }
  const key = `limit.${w.id}` as MessageKey;
  const s = t(key);
  return s === key ? w.id.replace(/_/g, ' ') : s;
};

const limitColor = (pct: number) => (pct >= 90 ? 'bg-danger' : pct >= 70 ? 'bg-warn' : 'bg-ok');

function Bar({ pct, className = 'h-1.5' }: { pct: number; className?: string }) {
  const v = Math.min(100, Math.max(0, pct));
  return (
    <div className={`overflow-hidden rounded-full bg-sunken ${className}`}>
      <div className={`h-full rounded-full ${limitColor(v)}`} style={{ width: `${v}%` }} />
    </div>
  );
}

function LimitRow({ w }: { w: LimitWindow }) {
  return (
    <div className="px-3.5 py-3">
      <div className="mb-1.5 flex items-baseline gap-2 text-sm">
        <span className="flex-1">{limitLabel(w)}</span>
        <span className="font-mono font-semibold">{w.utilization}%</span>
      </div>
      <Bar pct={w.utilization} />
      {w.resetsAt ? <div className="mt-1.5 text-xs text-muted">{t('harness.resets', { time: timeUntil(w.resetsAt) })}</div> : null}
    </div>
  );
}

const fmtDate = (iso: string) => new Date(iso).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' });

function useAct(reload: () => Promise<void>) {
  const [error, setError] = useState<string | null>(null);
  const act = async (fn: () => Promise<unknown>) => {
    setError(null);
    try {
      await fn();
      await reload();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };
  return { error, setError, act };
}

// ---------- one card for every agent: rendered by capabilities, not by agent kind ----------

function AgentCard({ info }: { info: HarnessInfo }) {
  const a = info.auth;
  const signedIn = Boolean(a?.loggedIn);
  const limits = info.limits;
  const main = limits?.[0];
  const asOf = info.limitsAsOf ? t('harness.limitsAsOf', { time: timeAgo(info.limitsAsOf) }) : undefined;
  const providers = [...new Set(info.accounts.map((x) => x.provider ?? ''))];
  const u = info.usage;

  // stats row: the first three the agent can fill
  const cells: ReactNode[] = [];
  if (a && signedIn && a.plan !== undefined) cells.push(<Stat key="plan" label={t('harness.plan')} value={a.plan ?? '—'} sub={a.planDetail ?? undefined} />);
  if (limits && signedIn)
    cells.push(
      <Stat key="limits" label={main ? limitLabel(main) : t('harness.limits')}>
        {main ? (
          <>
            <div className="mt-1 flex items-baseline gap-2">
              <span className="text-[15px] font-semibold">{main.utilization}%</span>
              <span className="truncate text-xs text-muted">{main.resetsAt ? t('harness.resets', { time: timeUntil(main.resetsAt) }) : ''}</span>
            </div>
            <Bar pct={main.utilization} className="mt-2 h-1.5" />
            {asOf ? <div className="mt-1.5 text-xs text-muted">{asOf}</div> : null}
          </>
        ) : (
          <div className="mt-1 text-xs text-muted">{t('harness.noLimits')}</div>
        )}
      </Stat>,
    );
  if (a)
    cells.push(
      <Stat
        key="auth"
        label={t('harness.signedIn')}
        value={signedIn ? (a.method ?? t('harness.on')) : t('harness.no')}
        sub={signedIn && a.expiresAt ? t('harness.until', { date: fmtDate(a.expiresAt) }) : undefined}
      />,
    );
  if (info.accountMode === 'providers')
    cells.push(<Stat key="providers" label={t('harness.providers')} value={providers.length} sub={info.accounts.map((x) => x.providerName).filter(Boolean).join(', ') || undefined} />);
  cells.push(
    <Stat
      key="models"
      label={t('harness.models')}
      value={info.models.length || '—'}
      sub={info.defaultModel ? t('harness.defaultIs', { model: info.defaultModel.split('/').pop() ?? '' }) : info.models.length ? undefined : t('harness.modelsAfterRun')}
    />,
  );
  if (u)
    cells.push(
      <Stat
        key="usage"
        label={t('harness.stats')}
        value={u.tokens ? `${u.tokens} ${t('harness.tokens')}` : '—'}
        sub={t('harness.statsShort', { sessions: u.sessions ?? '—', prompts: u.prompts ?? '—' })}
      />,
    );
  cells.push(<Stat key="protocol" label={t('harness.protocol')} value={info.protocol.toUpperCase()} sub={info.pinned ? t('harness.pinned') : undefined} />);

  return (
    <HarnessCard kind={info.kind} title={info.label} info={info} stats={<>{cells.slice(0, 3)}</>}>
      {info.accountMode === 'providers' ? <ProviderAccounts info={info} /> : <AgentAccount info={info} />}
      {limits && signedIn && limits.length > 1 ? (
        <Section title={t('harness.otherLimits')} aside={asOf}>
          <div className="divide-y divide-line/50 rounded-xl border border-line/60">
            {limits.slice(1).map((w) => (
              <LimitRow key={w.id} w={w} />
            ))}
          </div>
        </Section>
      ) : null}
      {info.defaultModelEdit === 'options' && info.models.length ? (
        <DefaultModelList info={info} />
      ) : info.defaultModelEdit === 'search' ? (
        <DefaultModelSearch info={info} />
      ) : info.models.length ? (
        <Section title={t('harness.models')}>
          <div className="flex flex-wrap gap-1.5">
            {info.models.map((m) => (
              <span key={m.id} className="rounded-md border border-line px-2 py-0.5 font-mono text-xs">
                {m.label}
              </span>
            ))}
          </div>
        </Section>
      ) : null}
    </HarnessCard>
  );
}

const defaultModelHint = (kind: AgentKind) => tOptional(`harness.defaultModelHint.${kind}`) ?? t('harness.defaultModelHint');

/** The agent login (accountMode: single): details and sign out, or sign in. */
function AgentAccount({ info }: { info: HarnessInfo }) {
  const { run } = useHarness();
  const a = info.auth;
  const has = (id: HarnessAction['id']) => info.actions.some((x) => x.id === id);
  const login = info.actions.find((x) => x.id === 'login');
  const signOut = (
    <ConfirmButton
      icon={<LogOut className="size-3.5" />}
      label={t('harness.signOut')}
      confirmLabel={t('harness.signOutConfirm')}
      onConfirm={() => run(info.kind, 'logout')}
    />
  );
  // the agent reports account details: show them instead of the bare account row
  if (a?.loggedIn && a.email !== undefined)
    return (
      <Section title={t('harness.account')}>
        <Rows
          items={[
            [t('harness.email'), a.email ?? '—'],
            ...(a.org ? ([[t('harness.org'), a.org]] as [string, ReactNode][]) : []),
            ...(a.extraUsage != null ? ([[t('harness.extraUsage'), a.extraUsage ? t('harness.on') : t('harness.off')]] as [string, ReactNode][]) : []),
          ]}
        />
        {has('logout') ? <div className="mt-2">{signOut}</div> : null}
      </Section>
    );
  if (!info.accounts.length) return null;
  return (
    <Section title={t('harness.account')}>
      <ul className="divide-y divide-line/50 rounded-xl border border-line/60">
        {info.accounts.map((acc) => (
          <AccountRow key={acc.id} a={{ ...acc, active: false }}>
            {acc.loggedIn ? (
              has('logout') ? signOut : null
            ) : (
              <>
                {login ? (
                  <RowButton onClick={() => run(info.kind, 'login')}>
                    <LogIn className="size-3.5" /> {t('harness.signIn')}
                  </RowButton>
                ) : null}
                {has('login-console') ? (
                  <RowButton onClick={() => run(info.kind, 'login-console')} title={tOptional(`harness.consoleHint.${info.kind}`) ?? undefined}>
                    {t('harness.console')}
                  </RowButton>
                ) : null}
              </>
            )}
          </AccountRow>
        ))}
      </ul>
      {login && !a?.loggedIn ? (
        <div className="mt-2">
          <CommandLine action={login} onRun={() => run(info.kind, 'login')} runLabel={t('harness.signIn')} />
        </div>
      ) : null}
      {a?.loggedIn ? <p className="mt-2 text-xs text-muted">{t('harness.switchHint')}</p> : null}
    </Section>
  );
}

/** Model provider credentials (accountMode: providers): switch, sign out, connect another. */
function ProviderAccounts({ info }: { info: HarnessInfo }) {
  const { run, reload } = useHarness();
  const { error, act } = useAct(reload);
  return (
    <Section title={t('harness.providers')} aside={t('harness.providersHint')}>
      {info.accounts.length ? (
        <ul className="divide-y divide-line/50 rounded-xl border border-line/60">
          {info.accounts.map((acc) => {
            const many = info.accounts.filter((x) => x.provider === acc.provider).length > 1;
            return (
              <AccountRow
                key={acc.id}
                a={{ ...acc, label: acc.providerName ?? acc.label, detail: many ? `${acc.label} · ${acc.detail ?? ''}` : acc.detail, active: many && acc.active }}
                icon={<ProviderIcon name={acc.provider ?? ''} />}
              >
                {many && !acc.active ? (
                  <RowButton onClick={() => act(() => api.harnessAccount(info.kind, acc.provider!, acc.id, 'switch'))}>
                    <Repeat className="size-3.5" /> {t('harness.use')}
                  </RowButton>
                ) : null}
                <ConfirmButton
                  icon={<LogOut className="size-3.5" />}
                  label={t('harness.signOut')}
                  confirmLabel={t('harness.signOutConfirm')}
                  onConfirm={() => act(() => api.harnessAccount(info.kind, acc.provider!, acc.id, 'logout'))}
                />
              </AccountRow>
            );
          })}
        </ul>
      ) : (
        <p className="text-sm text-muted">{t('harness.noAccounts')}</p>
      )}
      {info.actions.some((x) => x.id === 'login') ? (
        <Button size="sm" className="mt-2" onClick={() => run(info.kind, 'login')}>
          <Plus className="size-3.5" /> {t('harness.connectProvider')}
        </Button>
      ) : null}
      {error ? (
        <div className="mt-2">
          <ErrorLine error={error} />
        </div>
      ) : null}
    </Section>
  );
}

/** Default model picked from the agent's own short list (the `default` option clears it). */
function DefaultModelList({ info }: { info: HarnessInfo }) {
  const { reload } = useHarness();
  const { error, act } = useAct(reload);
  const [busy, setBusy] = useState<string | null>(null);
  const current = info.defaultModel;
  const isCurrent = (m: ModelOption) => (current ? m.id === current || m.resolved === current : m.id === 'default');
  const known = !current || info.models.some(isCurrent);
  const pick = async (m: ModelOption) => {
    if (isCurrent(m)) return;
    setBusy(m.id);
    await act(() => api.harnessDefaultModel(info.kind, m.id === 'default' ? null : m.id));
    setBusy(null);
  };
  return (
    <Section title={t('harness.defaultModel')} aside={defaultModelHint(info.kind)}>
      <ul className="divide-y divide-line/50 rounded-xl border border-line/60">
        {!known ? (
          <li className="flex items-center gap-3 bg-accent/8 px-3.5 py-2">
            <span className="min-w-0 flex-1 font-mono text-sm">{current}</span>
            <Check className="size-4 shrink-0 text-accent" />
          </li>
        ) : null}
        {info.models.map((m) => {
          const on = isCurrent(m);
          return (
            <li key={m.id}>
              <button
                type="button"
                onClick={() => void pick(m)}
                disabled={busy !== null}
                className={`flex w-full flex-wrap items-baseline gap-x-3 gap-y-0.5 px-3.5 py-2 text-left transition hover:bg-sunken disabled:cursor-wait ${on ? 'bg-accent/8' : ''}`}
              >
                <span className={`text-sm font-medium ${on ? 'text-accent' : ''}`}>{m.label}</span>
                {m.description ? <span className="min-w-0 flex-1 text-xs text-muted">{m.description}</span> : <span className="flex-1" />}
                {m.resolved ? <span className="font-mono text-[11px] text-muted">{m.resolved}</span> : null}
                <span className="flex size-4 shrink-0 self-center">
                  {busy === m.id ? <Spinner /> : on ? <Check className="size-4 text-accent" /> : null}
                </span>
              </button>
            </li>
          );
        })}
      </ul>
      {error ? (
        <div className="mt-2">
          <ErrorLine error={error} />
        </div>
      ) : null}
    </Section>
  );
}

/** Default model chosen from a long list with search (or any id). */
function DefaultModelSearch({ info }: { info: HarnessInfo }) {
  const { reload } = useHarness();
  const [model, setModel] = useState<string | undefined>(info.defaultModel ?? undefined);
  const [saving, setSaving] = useState(false);
  const { error, act } = useAct(reload);
  return (
    <Section title={t('harness.defaultModel')} aside={defaultModelHint(info.kind)}>
      <ModelPicker models={info.models.map((m) => m.id)} value={model} onChange={setModel} />
      <div className="mt-2 flex items-center gap-2">
        <span className="min-w-0 flex-1 truncate font-mono text-xs text-muted">{model ?? '—'}</span>
        <Button
          size="sm"
          variant="primary"
          loading={saving}
          disabled={(model ?? null) === (info.defaultModel ?? null)}
          onClick={async () => {
            setSaving(true);
            await act(() => api.harnessDefaultModel(info.kind, model ?? null));
            setSaving(false);
          }}
        >
          {t('common.save')}
        </Button>
      </div>
      {error ? (
        <div className="mt-2">
          <ErrorLine error={error} />
        </div>
      ) : null}
    </Section>
  );
}
