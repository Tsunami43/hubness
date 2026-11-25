import { useEffect, useMemo, useState } from 'react';
import { RotateCw, ScrollText } from 'lucide-react';
import type { UnitInfo } from '@hubness/shared';
import { api } from '../../lib/api';
import { useApi } from '../../lib/query';
import { AskButton } from '../../components/AskButton';
import { Button, Empty, Spinner } from '../../components/ui';
import { t } from '../../i18n';
import { Header, Filter, Chips, Dot, LogsSheet, Result } from './parts';
import { usePlatform } from './platform';

function unitDot(u: UnitInfo) {
  if (u.active === 'failed') return 'bg-danger';
  if (u.active === 'active') return 'bg-ok';
  if (u.active === 'activating' || u.active === 'reloading') return 'bg-warn animate-pulse';
  return 'bg-muted/40';
}

export function ServicesSection() {
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
