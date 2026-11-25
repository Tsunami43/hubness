import { useEffect, useMemo, useState } from 'react';
import { useSearchParams } from 'react-router';
import { Boxes, Play, RotateCw, ScrollText, Square, Trash2 } from 'lucide-react';
import type { ContainerInfo } from '@hubness/shared';
import { api } from '../../lib/api';
import { confirmDialog } from '../../components/Confirm';
import { useApi } from '../../lib/query';
import { AskButton } from '../../components/AskButton';
import { Button, Empty, ErrorLine, Spinner } from '../../components/ui';
import { t } from '../../i18n';
import { Header, Filter, Chips, Dot, LogsSheet, Result } from './parts';

function containerDot(c: ContainerInfo) {
  if (c.state === 'running') return c.health === 'unhealthy' ? 'bg-warn' : 'bg-ok';
  if (c.state === 'restarting') return 'bg-danger animate-pulse';
  return 'bg-muted/40';
}

type DockerTab = 'containers' | 'networks' | 'volumes';

/** Docker: containers, networks, volumes — each tab loads only when opened. */
export function DockerSection() {
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
