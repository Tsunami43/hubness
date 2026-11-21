import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { Link, Navigate, useParams, useSearchParams } from 'react-router';
import {
  Activity,
  Boxes,
  Cog,
  Package,
  Play,
  RotateCw,
  ScrollText,
  Search,
  Square,
  Trash2,
} from 'lucide-react';
import type { AskTarget, ContainerInfo, HostInfo, SystemOverview, SystemPlatform, UnitInfo } from '@hubness/shared';
import { api } from '../lib/api';
import { confirmDialog } from '../components/Confirm';
import { timeAgo } from '../lib/format';
import { useApi } from '../lib/query';
import { AskButton } from '../components/AskButton';
import { OsLogo } from '../components/OsLogo';
import { SubmenuIndex, SubmenuLayout, type SubmenuItem } from '../components/SubmenuLayout';
import { Button, Empty, ErrorLine, Sheet, Spinner } from '../components/ui';
import { t } from '../i18n';
import { askAboutLogs } from '../lib/askDraft';

/**
 * System: server monitoring with a submenu and search. The menu asks only the (static)
 * platform; every section loads (and refreshes) its own data only while it is open.
 */
// the platform never changes while the server runs: one request per page load
let platformReq: Promise<SystemPlatform> | null = null;
function usePlatform(): SystemPlatform | null {
  const [p, setP] = useState<SystemPlatform | null>(null);
  useEffect(() => {
    platformReq ??= api.sysPlatform().catch((e) => ((platformReq = null), Promise.reject(e)));
    let alive = true;
    platformReq.then((v) => alive && setP(v)).catch(() => {});
    return () => {
      alive = false;
    };
  }, []);
  return p;
}

export function SystemLayout() {
  const platform = usePlatform();
  const items: SubmenuItem[] = [
    { to: 'overview', label: t('sys.overview'), hint: t('sys.overviewMenuHint'), icon: <Activity className="size-4 text-accent" />, keywords: ['cpu', 'memory', 'ram', 'disk', 'load', 'uptime', 'swap'] },
    { to: 'docker', label: 'Docker', hint: t('sys.dockerMenuHint'), icon: <Boxes className="size-4 text-accent" />, keywords: ['containers', 'compose', 'logs', 'images'] },
    { to: 'services', label: t('sys.services'), hint: platform ? (platform.services ?? t('sys.unavailable')) : '', icon: <Cog className="size-4 text-accent" />, keywords: ['systemd', 'launchd', 'launchctl', 'units', 'agents', 'journal', 'logs', 'failed'] },
    { to: 'packages', label: t('sys.packages'), hint: t('sys.packagesMenuHint'), icon: <Package className="size-4 text-accent" />, keywords: ['pacman', 'aur', 'yay', 'paru', 'cargo', 'rustup', 'npm', 'pip', 'uv', 'go', 'apt', 'dnf', 'flatpak', 'snap', 'brew', 'updates', 'installed', 'upgrade'] },
  ];
  return <SubmenuLayout base="/system" title={t('nav.system')} items={items} searchPlaceholder={t('sys.search')} />;
}

export function SystemIndex() {
  return <SubmenuIndex to="overview" />;
}

export function SystemSection() {
  const { section } = useParams();
  switch (section) {
    case 'overview':
      return <OverviewSection />;
    case 'docker':
      return <DockerSection />;
    case 'services':
      return <ServicesSection />;
    case 'packages':
      return <PackagesSection />;
    default:
      return <Navigate to="/system" replace />;
  }
}

// ---------- shared parts ----------

function fmtUptime(sec: number) {
  const d = Math.floor(sec / 86400);
  const h = Math.floor((sec % 86400) / 3600);
  const m = Math.floor((sec % 3600) / 60);
  return d ? `${d}d ${h}h` : h ? `${h}h ${m}m` : `${m}m`;
}

function Header({ title, hint, actions }: { title: string; hint?: ReactNode; actions?: ReactNode }) {
  return (
    <div className="mb-4 flex flex-wrap items-end gap-3">
      <div className="min-w-0 flex-1">
        <h2 className="text-lg font-semibold">{title}</h2>
        {hint ? <p className="text-sm text-muted">{hint}</p> : null}
      </div>
      {actions}
    </div>
  );
}

function Filter({ value, onChange, placeholder }: { value: string; onChange: (v: string) => void; placeholder: string }) {
  return (
    <div className="relative min-w-48 flex-1">
      <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted" />
      <input
        type="search"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className="w-full rounded-xl border border-line bg-surface py-2 pr-3 pl-9 text-sm outline-none placeholder:text-muted/70 focus:border-accent"
      />
    </div>
  );
}

function Chips<T extends string>({ value, onChange, options }: { value: T; onChange: (v: T) => void; options: [T, string][] }) {
  return (
    <div className="flex gap-1 rounded-xl border border-line bg-surface p-1 text-sm">
      {options.map(([id, label]) => (
        <button
          key={id}
          type="button"
          onClick={() => onChange(id)}
          className={`rounded-lg px-2.5 py-1 whitespace-nowrap ${value === id ? 'bg-sunken text-fg' : 'text-muted hover:text-fg'}`}
        >
          {label}
        </button>
      ))}
    </div>
  );
}

function Bar({ pct }: { pct: number }) {
  const v = Math.min(100, Math.max(0, pct));
  const color = v >= 90 ? 'bg-danger' : v >= 75 ? 'bg-warn' : 'bg-ok';
  return (
    <div className="h-1.5 overflow-hidden rounded-full bg-sunken">
      <div className={`h-full rounded-full ${color}`} style={{ width: `${v}%` }} />
    </div>
  );
}

const Dot = ({ cls }: { cls: string }) => <span className={`inline-block size-2 shrink-0 rounded-full ${cls}`} />;

function LogsSheet({ title, subject, load, onClose }: { title: string | null; subject?: string; load: () => Promise<string>; onClose: () => void }) {
  const [logs, setLogs] = useState<string | null>(null);
  useEffect(() => {
    if (!title) return;
    setLogs(null);
    load().then(setLogs, (e) => setLogs(String(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [title]);
  return (
    <Sheet
      open={title !== null}
      onClose={onClose}
      title={title ?? ''}
      footer={logs && subject ? <AskButton text={askAboutLogs(subject, logs)} variant="primary" label={t('ask.logs')} /> : undefined}
    >
      {logs === null ? (
        <Spinner />
      ) : (
        <pre className="max-h-[65dvh] overflow-auto rounded-lg bg-sunken p-3 font-mono text-[11.5px] leading-relaxed whitespace-pre-wrap break-all">
          {logs || t('sys.noLogs')}
        </pre>
      )}
    </Sheet>
  );
}

function Result({ r }: { r: { ok: boolean; output: string; name: string } | null }) {
  if (!r) return null;
  return (
    <div className={`mb-3 rounded-lg px-3 py-2 text-sm ${r.ok ? 'bg-ok/10 text-ok' : 'border border-danger/40 text-danger'}`}>
      <span className="font-medium">{r.name}:</span> {r.output || (r.ok ? 'OK' : t('git.failed'))}
    </div>
  );
}

// ---------- Overview ----------

function Tile({ label, value, sub, children, to }: { label: string; value: ReactNode; sub?: ReactNode; children?: ReactNode; to?: string }) {
  const inner = (
    <>
      <div className="text-xs text-muted">{label}</div>
      <div className="mt-1 text-2xl font-semibold">{value}</div>
      {children ? <div className="mt-2">{children}</div> : null}
      {sub ? <div className="mt-1.5 text-xs leading-5 break-words text-muted">{sub}</div> : null}
    </>
  );
  const cls = 'block min-w-0 rounded-2xl border border-line bg-surface px-4 py-3.5';
  return to ? (
    <Link to={to} className={`${cls} hover:border-muted/50`}>
      {inner}
    </Link>
  ) : (
    <div className={cls}>{inner}</div>
  );
}

const tempCls = (c: number) => (c >= 85 ? 'text-danger' : c >= 70 ? 'text-warn' : 'text-fg');

/** OS logo with name, machine and uptime. */
function HostHeader({ o }: { o: SystemOverview }) {
  const i: HostInfo = o.info;
  return (
    <section className="flex items-center gap-4 border border-line bg-surface px-4 py-4">
      <OsLogo id={i.osId} like={i.osLike} className="size-12 sm:size-14" />
      <div className="min-w-0">
        <div className="truncate text-base font-semibold">
          {i.osName} <span className="font-normal text-muted">{i.arch}</span>
        </div>
        <div className="truncate text-xs text-muted">{[o.hostname, i.host].filter(Boolean).join(' · ')}</div>
        <div className="truncate text-xs text-muted">
          {i.kernel} · {t('sys.uptime', { time: fmtUptime(o.uptimeSec) })}
        </div>
      </div>
    </section>
  );
}

function OverviewSection() {
  // polled only while this section is open
  const { data: o, reload, error } = useApi(['system', 'overview'], () => api.sysOverview());
  useEffect(() => {
    const id = setInterval(() => void reload(), 10_000);
    return () => clearInterval(id);
  }, [reload]);
  if (!o) return error ? <ErrorLine error={error} /> : <Spinner />;
  const cpuTemp = o.temps.find((x) => x.kind === 'cpu');
  const usedGb = ((o.memory.totalMb - o.memory.availableMb) / 1024).toFixed(1);
  return (
    <div>
      <Header title={t('sys.overview')} hint={t('sys.overviewHint')} />
      {o.info.platform === 'other' ? <p className="mb-3 border border-warn/50 px-3 py-2 text-sm text-warn">{t('sys.unsupported')}</p> : null}
      <HostHeader o={o} />
      <div className="mt-3 grid grid-cols-2 gap-3 sm:grid-cols-4">
        <Tile
          label="CPU"
          value={`${o.cpuPercent ?? '—'}%`}
          sub={
            <>
              {o.info.cpu ? <div>{o.info.cpu}</div> : null}
              <div>{t('sys.threads', { n: o.cpus })}</div>
            </>
          }
        >
          <Bar pct={o.cpuPercent ?? 0} />
        </Tile>
        <Tile
          label="RAM"
          value={`${o.memory.usedPercent}%`}
          sub={
            <>
              <div>{usedGb} / {(o.memory.totalMb / 1024).toFixed(1)} GB</div>
              {o.swap.totalMb ? (
                <div>
                  swap {(o.swap.usedMb / 1024).toFixed(1)} / {(o.swap.totalMb / 1024).toFixed(1)} GB
                </div>
              ) : null}
            </>
          }
        >
          <Bar pct={o.memory.usedPercent} />
        </Tile>
        {o.gpus.map((g, n) => (
          <Tile key={n} label={o.gpus.length > 1 ? `GPU ${n + 1}` : 'GPU'} value={g.usagePercent === null ? '—' : `${g.usagePercent}%`} sub={g.name}>
            {g.usagePercent === null ? null : <Bar pct={g.usagePercent} />}
          </Tile>
        ))}
        {o.temps.length ? (
          <Tile
            label={t('sys.temperature')}
            value={<span className={tempCls(cpuTemp?.celsius ?? o.temps[0].celsius)}>{Math.round(cpuTemp?.celsius ?? o.temps[0].celsius)}°C</span>}
            sub={o.temps.map((x) => (
              <div key={x.label} className="flex justify-between gap-2">
                <span>{x.label}</span>
                <span className={tempCls(x.celsius)}>{Math.round(x.celsius)}°C</span>
              </div>
            ))}
          />
        ) : null}
      </div>
      <h3 className="mt-6 mb-2 text-sm font-semibold">{t('sys.disks')}</h3>
      <ul className="divide-y divide-line/50 rounded-2xl border border-line bg-surface">
        {o.disks.map((d) => (
          <li key={d.mount} className="px-4 py-3">
            <div className="mb-1.5 flex items-baseline gap-2 text-sm">
              <span className="font-mono">{d.mount}</span>
              <span className="truncate text-xs text-muted">{d.device}</span>
              <span className="ml-auto shrink-0 text-muted">
                {d.usedGb} / {d.sizeGb} GB · <span className="font-semibold text-fg">{d.usedPercent}%</span>
              </span>
            </div>
            <Bar pct={d.usedPercent} />
          </li>
        ))}
      </ul>
    </div>
  );
}

// ---------- Docker ----------

function containerDot(c: ContainerInfo) {
  if (c.state === 'running') return c.health === 'unhealthy' ? 'bg-warn' : 'bg-ok';
  if (c.state === 'restarting') return 'bg-danger animate-pulse';
  return 'bg-muted/40';
}

type DockerTab = 'containers' | 'networks' | 'volumes';

/** Docker: containers, networks, volumes — each tab loads only when opened. */
function DockerSection() {
  const [params, setParams] = useSearchParams();
  const tab: DockerTab = params.get('dt') === 'networks' ? 'networks' : params.get('dt') === 'volumes' ? 'volumes' : 'containers';
  return (
    <div>
      <Header title="Docker" hint={t('sys.dockerSectionHint')} />
      <div className="mb-4 flex gap-1 border-b border-line">
        {(['containers', 'networks', 'volumes'] as const).map((k) => (
          <button
            key={k}
            type="button"
            data-jump
            onClick={() => setParams(k === 'containers' ? {} : { dt: k }, { replace: true })}
            className={`border-b-2 px-3 py-1.5 text-sm ${tab === k ? 'border-accent text-fg' : 'border-transparent text-muted hover:text-fg'}`}
          >
            {t(`sys.docker.${k}`)}
          </button>
        ))}
      </div>
      {tab === 'containers' ? <ContainersTab /> : tab === 'networks' ? <NetworksTab /> : <VolumesTab />}
    </div>
  );
}

/** Two-step remove: the first click arms, the second removes (no browser dialogs). */
function RemoveButton({ busy, onRemove, label }: { busy?: boolean; onRemove: () => void; label?: string }) {
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    if (!armed) return;
    const timer = setTimeout(() => setArmed(false), 4000);
    return () => clearTimeout(timer);
  }, [armed]);
  return (
    <Button size="sm" variant={armed ? 'danger' : 'ghost'} loading={busy} onClick={() => (armed ? (setArmed(false), onRemove()) : setArmed(true))}>
      <Trash2 className="size-3.5" /> {armed ? t('sys.removeConfirm') : (label ?? t('sys.remove'))}
    </Button>
  );
}

function ContainersTab() {
  const { data, reload, loading } = useApi(['system', 'containers'], () => api.containers());
  const [q, setQ] = useState('');
  const [filter, setFilter] = useState<'all' | 'running' | 'stopped'>('running');
  const [busy, setBusy] = useState<string | null>(null);
  const [result, setResult] = useState<{ ok: boolean; output: string; name: string } | null>(null);
  const [logs, setLogs] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const list = useMemo(() => {
    const all = data?.containers ?? [];
    return all
      .filter((c) => (filter === 'all' ? true : filter === 'running' ? c.state === 'running' || c.state === 'restarting' : c.state !== 'running'))
      .filter((c) => !q.trim() || `${c.name} ${c.image}`.toLowerCase().includes(q.trim().toLowerCase()));
  }, [data, q, filter]);

  const act = async (c: ContainerInfo, action: string) => {
    setBusy(`${c.name}:${action}`);
    try {
      const r = await api.containerAction(c.name, action);
      setResult({ ...r, name: `${action} ${c.name}` });
      await reload();
    } finally {
      setBusy(null);
    }
  };

  const remove = async (c: ContainerInfo) => {
    setBusy(`${c.name}:rm`);
    setError(null);
    try {
      const r = await api.containerRemove(c.name);
      setResult({ ...r, name: `${t('sys.remove')} ${c.name}` });
      await reload();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(null);
    }
  };

  if (data && !data.available) return <Empty icon={<Boxes className="size-8" />} title={t('sys.noDocker')} hint={t('sys.noDockerHint')} />;

  return (
    <div>
      {data ? <p className="mb-3 text-sm text-muted">{t('sys.dockerHint', { running: data.containers.filter((c) => c.state === 'running').length, total: data.containers.length })}</p> : null}
      <div className="mb-3 flex flex-wrap gap-2">
        <Filter value={q} onChange={setQ} placeholder={t('sys.filterContainers')} />
        <Chips value={filter} onChange={setFilter} options={[['running', t('sys.running')], ['stopped', t('sys.stopped')], ['all', t('sys.all')]]} />
      </div>
      <Result r={result} />
      <ErrorLine error={error} />
      {loading && !data ? (
        <Spinner />
      ) : list.length ? (
        <ul className="divide-y divide-line/50 rounded-2xl border border-line bg-surface">
          {list.map((c) => (
            <li key={c.id} className="px-4 py-3">
              <div className="flex items-start gap-3">
                <span className="mt-2">
                  <Dot cls={containerDot(c)} />
                </span>
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-baseline gap-x-2">
                    <span className="font-medium break-all">{c.name}</span>
                    <span className="truncate font-mono text-xs text-muted">{c.image}</span>
                  </div>
                  <div className="text-xs text-muted">
                    {c.status}
                    {c.restartCount ? ` · ${t('sys.restarts', { n: c.restartCount })}` : ''}
                    {c.memLimitMb ? ` · mem ${c.memLimitMb} MB` : ''}
                    {c.oomKilled ? ' · OOM' : ''}
                  </div>
                </div>
              </div>
              <div className="mt-2 flex flex-wrap gap-1.5 pl-5">
                <Button size="sm" variant="ghost" onClick={() => setLogs(c.name)}>
                  <ScrollText className="size-3.5" /> {t('sys.logs')}
                </Button>
                {c.state === 'running' ? (
                  <>
                    <Button size="sm" variant="ghost" loading={busy === `${c.name}:restart`} onClick={() => act(c, 'restart')}>
                      <RotateCw className="size-3.5" /> {t('sys.restart')}
                    </Button>
                    <Button size="sm" variant="ghost" loading={busy === `${c.name}:stop`} onClick={async () => (await confirmDialog({ message: t('sys.confirmStop', { name: c.name }), confirmLabel: t('sys.stop'), danger: true })) && act(c, 'stop')}>
                      <Square className="size-3.5" /> {t('sys.stop')}
                    </Button>
                  </>
                ) : (
                  <>
                    <Button size="sm" variant="ghost" loading={busy === `${c.name}:start`} onClick={() => act(c, 'start')}>
                      <Play className="size-3.5" /> {t('sys.start')}
                    </Button>
                    {c.state !== 'restarting' && c.state !== 'paused' ? <RemoveButton busy={busy === `${c.name}:rm`} onRemove={() => void remove(c)} /> : null}
                  </>
                )}
                <AskButton target={{ kind: 'container', id: c.name }} variant="ghost" />
              </div>
            </li>
          ))}
        </ul>
      ) : (
        <Empty title={t('sys.nothing')} />
      )}
      <LogsSheet title={logs ? `${t('sys.logs')} · ${logs}` : null} subject={logs ? t('ask.subjectContainer', { name: logs }) : undefined} load={async () => (await api.containerLogs(logs!)).logs} onClose={() => setLogs(null)} />
    </div>
  );
}

function NetworksTab() {
  const { data, reload, loading, error } = useApi(['system', 'networks'], () => api.networks());
  const [busy, setBusy] = useState<string | null>(null);
  const [opError, setOpError] = useState<string | null>(null);
  const [result, setResult] = useState<{ ok: boolean; output: string; name: string } | null>(null);
  const [q, setQ] = useState('');
  const remove = async (name: string) => {
    setBusy(name);
    setOpError(null);
    try {
      setResult({ ...(await api.networkRemove(name)), name: `${t('sys.remove')} ${name}` });
      await reload();
    } catch (e) {
      setOpError((e as Error).message);
    } finally {
      setBusy(null);
    }
  };
  if (data && !data.available) return <Empty icon={<Boxes className="size-8" />} title={t('sys.noDocker')} hint={t('sys.noDockerHint')} />;
  const needle = q.trim().toLowerCase();
  const list = (data?.networks ?? []).filter((n) => !needle || `${n.name} ${n.project ?? ''} ${n.containers.join(' ')}`.toLowerCase().includes(needle));
  return (
    <div>
      <div className="mb-3 flex">
        <Filter value={q} onChange={setQ} placeholder={t('sys.filterNetworks')} />
      </div>
      <Result r={result} />
      <ErrorLine error={error ?? opError} />
      {loading && !data ? (
        <Spinner />
      ) : (
        <ul className="divide-y divide-line/50 border border-line bg-surface">
          {list.map((n) => (
            <li key={n.id} className="flex flex-wrap items-center gap-3 px-4 py-3">
              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-baseline gap-x-2">
                  <span className="font-medium break-all">{n.name}</span>
                  <span className="font-mono text-xs text-muted">{n.driver}</span>
                  {n.builtin ? <span className="border border-line px-1 text-[11px] text-muted">{t('sys.builtin')}</span> : null}
                  {n.internal ? <span className="border border-line px-1 text-[11px] text-muted">internal</span> : null}
                </div>
                <div className="text-xs text-muted">
                  {[n.subnet, n.project ? `compose: ${n.project}` : null, n.containers.length ? t('sys.attached', { names: n.containers.join(', ') }) : t('sys.noAttached')].filter(Boolean).join(' · ')}
                </div>
              </div>
              {!n.builtin && !n.containers.length ? <RemoveButton busy={busy === n.name} onRemove={() => void remove(n.name)} /> : null}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function VolumesTab() {
  const { data, reload, loading, error } = useApi(['system', 'volumes'], () => api.volumes());
  const [busy, setBusy] = useState<string | null>(null);
  const [opError, setOpError] = useState<string | null>(null);
  const [result, setResult] = useState<{ ok: boolean; output: string; name: string } | null>(null);
  const [q, setQ] = useState('');
  const [filter, setFilter] = useState<'all' | 'used' | 'unused'>('all');
  const remove = async (name: string) => {
    setBusy(name);
    setOpError(null);
    try {
      setResult({ ...(await api.volumeRemove(name)), name: `${t('sys.remove')} ${name.slice(0, 24)}` });
      await reload();
    } catch (e) {
      setOpError((e as Error).message);
    } finally {
      setBusy(null);
    }
  };
  if (data && !data.available) return <Empty icon={<Boxes className="size-8" />} title={t('sys.noDocker')} hint={t('sys.noDockerHint')} />;
  const needle = q.trim().toLowerCase();
  const all = data?.volumes ?? [];
  const list = all
    .filter((v) => (filter === 'all' ? true : filter === 'used' ? v.links > 0 : v.links === 0))
    .filter((v) => !needle || `${v.name} ${v.project ?? ''}`.toLowerCase().includes(needle));
  return (
    <div>
      {data ? <p className="mb-3 text-sm text-muted">{t('sys.volumesHint', { total: all.length, unused: all.filter((v) => v.links === 0).length })}</p> : null}
      <div className="mb-3 flex flex-wrap gap-2">
        <Filter value={q} onChange={setQ} placeholder={t('sys.filterVolumes')} />
        <Chips value={filter} onChange={setFilter} options={[['all', t('sys.all')], ['used', t('sys.used')], ['unused', t('sys.unused')]]} />
      </div>
      <Result r={result} />
      <ErrorLine error={error ?? opError} />
      {loading && !data ? (
        <div className="flex items-center gap-2 text-sm text-muted">
          <Spinner /> {t('sys.volumesReading')}
        </div>
      ) : (
        <ul className="divide-y divide-line/50 border border-line bg-surface">
          {list.map((v) => (
            <li key={v.name} className="flex flex-wrap items-center gap-3 px-4 py-3">
              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-baseline gap-x-2">
                  <span className="font-mono text-sm break-all" title={v.name}>
                    {v.anonymous ? `${v.name.slice(0, 12)}…` : v.name}
                  </span>
                  {v.anonymous ? <span className="border border-line px-1 text-[11px] text-muted">{t('sys.anonymous')}</span> : null}
                </div>
                <div className="text-xs text-muted">
                  {[v.size, v.project ? `compose: ${v.project}` : null, v.links ? t('sys.usedBy', { n: v.links }) : t('sys.unusedVolume')].filter(Boolean).join(' · ')}
                </div>
              </div>
              {v.links === 0 ? <RemoveButton busy={busy === v.name} onRemove={() => void remove(v.name)} /> : null}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

// ---------- systemd ----------

function unitDot(u: UnitInfo) {
  if (u.active === 'failed') return 'bg-danger';
  if (u.active === 'active') return 'bg-ok';
  if (u.active === 'activating' || u.active === 'reloading') return 'bg-warn animate-pulse';
  return 'bg-muted/40';
}

function ServicesSection() {
  const platform = usePlatform();
  if (!platform) return <Spinner />;
  if (!platform.services)
    return (
      <div>
        <Header title={t('sys.services')} />
        <Empty title={t('sys.noServices')} />
      </div>
    );
  return <ServicesList launchd={platform.services === 'launchd'} />;
}

function ServicesList({ launchd }: { launchd: boolean }) {
  const { data, reload, loading } = useApi(['system', 'units'], () => api.units());
  const [q, setQ] = useState('');
  const [filter, setFilter] = useState<'failed' | 'active' | 'all'>('active');
  const [scope, setScope] = useState<'all' | 'system' | 'user'>('all');
  const [busy, setBusy] = useState<string | null>(null);
  const [result, setResult] = useState<{ ok: boolean; output: string; name: string } | null>(null);
  const [logs, setLogs] = useState<UnitInfo | null>(null);

  useEffect(() => {
    if (data?.some((u) => u.active === 'failed')) setFilter('failed');
  }, [data]);

  const list = useMemo(
    () =>
      (data ?? [])
        .filter((u) => (filter === 'all' ? true : filter === 'failed' ? u.active === 'failed' : u.active === 'active'))
        .filter((u) => scope === 'all' || u.scope === scope)
        .filter((u) => !q.trim() || `${u.name} ${u.description}`.toLowerCase().includes(q.trim().toLowerCase())),
    [data, q, filter, scope],
  );

  const act = async (u: UnitInfo, action: string) => {
    setBusy(`${u.scope}:${u.name}:${action}`);
    try {
      const r = await api.unitAction(u.scope, u.name, action);
      setResult({ ...r, name: `${action} ${u.name}` });
      await reload();
    } finally {
      setBusy(null);
    }
  };

  return (
    <div>
      <Header title={t('sys.services')} hint={launchd ? t('sys.servicesHintLaunchd') : t('sys.servicesHint')} />
      <div className="mb-3 flex flex-wrap gap-2">
        <Filter value={q} onChange={setQ} placeholder={t('sys.filterUnits')} />
        <Chips value={filter} onChange={setFilter} options={[['failed', t('sys.failed')], ['active', t('sys.active')], ['all', t('sys.all')]]} />
        {launchd ? null : <Chips value={scope} onChange={setScope} options={[['all', t('sys.all')], ['system', 'system'], ['user', 'user']]} />}
      </div>
      <Result r={result} />
      {loading && !data ? (
        <Spinner />
      ) : list.length ? (
        <ul className="divide-y divide-line/50 rounded-2xl border border-line bg-surface">
          {list.map((u) => (
            <li key={`${u.scope}:${u.name}`} className="px-4 py-3">
              <div className="flex items-start gap-3">
                <span className="mt-2">
                  <Dot cls={unitDot(u)} />
                </span>
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-baseline gap-x-2">
                    <span className="font-mono text-[13px] font-medium break-all">{u.name}</span>
                    {launchd ? null : <span className="rounded border border-line px-1.5 text-[11px] text-muted">{u.scope}</span>}
                  </div>
                  <div className="text-xs text-muted">
                    {[u.description, `${u.active}/${u.sub}`].filter(Boolean).join(' · ')}
                  </div>
                </div>
              </div>
              <div className="mt-2 flex flex-wrap gap-1.5 pl-5">
                <Button size="sm" variant="ghost" onClick={() => setLogs(u)}>
                  <ScrollText className="size-3.5" /> {t('sys.logs')}
                </Button>
                {u.scope === 'user' ? (
                  <Button size="sm" variant="ghost" loading={busy === `${u.scope}:${u.name}:restart`} onClick={() => act(u, 'restart')}>
                    <RotateCw className="size-3.5" /> {t('sys.restart')}
                  </Button>
                ) : null}
                <AskButton target={{ kind: 'unit', id: u.name, scope: u.scope }} variant="ghost" />
              </div>
            </li>
          ))}
        </ul>
      ) : (
        <Empty title={t('sys.nothing')} />
      )}
      <LogsSheet
        title={logs ? `${t('sys.logs')} · ${logs.name}` : null}
        subject={logs ? t('ask.subjectUnit', { name: logs.name, scope: logs.scope }) : undefined}
        load={async () => (await api.unitLogs(logs!.scope, logs!.name)).logs}
        onClose={() => setLogs(null)}
      />
    </div>
  );
}

// ---------- packages ----------

/** Packages: a tab per package manager on the server; each tab loads (server-cached) on open. */
function PackagesSection() {
  const { data: found, loading } = useApi(['system', 'package-managers'], () => api.packageManagers());
  const [params, setParams] = useSearchParams();
  const managers = found?.managers ?? [];
  const current = managers.find((m) => m.id === params.get('pm'))?.id ?? managers[0]?.id;
  if (loading && !found) return <Spinner />;
  if (!managers.length) return <Empty icon={<Package className="size-8" />} title={t('sys.noManagers')} />;
  return (
    <div className="space-y-4">
      <Header title={t('sys.packages')} hint={t('sys.packagesManagersHint')} />
      <div className="flex gap-1 overflow-x-auto border-b border-line [scrollbar-width:none]">
        {managers.map((m) => (
          <button
            key={m.id}
            type="button"
            data-jump
            onClick={() => setParams({ pm: m.id }, { replace: true })}
            className={`shrink-0 border-b-2 px-3 py-1.5 text-sm whitespace-nowrap ${current === m.id ? 'border-accent text-fg' : 'border-transparent text-muted hover:text-fg'}`}
          >
            {m.label}
          </button>
        ))}
      </div>
      {current ? <ManagerPackages key={current} manager={current} /> : null}
    </div>
  );
}

function ManagerPackages({ manager }: { manager: string }) {
  const [refresh, setRefresh] = useState(0);
  const { data, loading, error } = useApi(['system', 'packages', manager, refresh], () => api.packages(manager, refresh > 0));
  const [q, setQ] = useState('');
  // "ask the agent" knows how to inspect system packages (pacman -Qi)
  const askable = manager === 'pacman' || manager === 'aur';
  const ask = (name: string): AskTarget => ({ kind: 'package', id: name });

  if (loading && !data)
    return (
      <div className="flex items-center gap-2 text-sm text-muted">
        <Spinner /> {t('sys.packagesReading')}
      </div>
    );
  if (error) return <ErrorLine error={error} />;
  if (!data) return null;
  const needle = q.trim().toLowerCase();
  const list = data.packages.filter((p) => !needle || `${p.name} ${p.note ?? ''}`.toLowerCase().includes(needle));

  return (
    <div className="space-y-6">
      <div className="flex flex-wrap items-center gap-2 text-xs text-muted">
        <span>{t('sys.packagesCount', { n: data.installed })}</span>
        <span>·</span>
        <span>{t('sys.checked', { time: timeAgo(data.checkedAt) })}</span>
        <Button size="sm" variant="ghost" loading={loading} onClick={() => setRefresh((n) => n + 1)}>
          <RotateCw className="size-3.5" /> {t('files.refresh')}
        </Button>
      </div>
      <section>
        <h3 className="mb-1 text-sm font-semibold">{data.updates ? t('sys.updates', { n: data.updates.length }) : t('sys.updatesUnknown')}</h3>
        <p className="mb-2 text-xs text-muted">{data.updatesNote}</p>
        {data.updates?.length ? (
          <ul className="divide-y divide-line/50 border border-line bg-surface text-sm">
            {data.updates.map((p) => (
              <li key={p.name} className="flex items-center gap-3 px-4 py-2.5">
                <span className="min-w-0 flex-1 font-mono break-all">{p.name}</span>
                <span className="font-mono text-xs text-muted">
                  {p.version} → <span className="text-ok">{p.newVersion}</span>
                </span>
              </li>
            ))}
          </ul>
        ) : data.updates ? (
          <p className="text-sm text-muted">{t('sys.noUpdates')}</p>
        ) : null}
      </section>

      <section>
        <h3 className="mb-2 text-sm font-semibold">{data.listTitle === 'explicit' ? t('sys.explicit', { n: data.packages.length }) : t('sys.installedList', { n: data.packages.length })}</h3>
        <div className="mb-2 flex">
          <Filter value={q} onChange={setQ} placeholder={t('sys.filterPackages')} />
        </div>
        <ul className="divide-y divide-line/50 border sm:max-h-[50dvh] sm:overflow-y-auto border-line bg-surface text-sm">
          {list.map((p) => (
            <li key={p.name} className="flex items-center gap-3 px-4 py-2">
              <span className="min-w-0 flex-1 truncate">
                <span className="font-mono">{p.name}</span>
                {p.note ? <span className="ml-2 text-xs text-muted">{p.note}</span> : null}
              </span>
              <span className="shrink-0 font-mono text-xs text-muted">{p.version}</span>
              {askable ? <AskButton target={ask(p.name)} variant="ghost" label="" /> : null}
            </li>
          ))}
        </ul>
      </section>
    </div>
  );
}
