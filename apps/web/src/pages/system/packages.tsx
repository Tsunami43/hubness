import { useState } from 'react';
import { useSearchParams } from 'react-router';
import { Package, RotateCw } from 'lucide-react';
import type { AskTarget } from '@hubness/shared';
import { api } from '../../lib/api';
import { timeAgo } from '../../lib/format';
import { useApi } from '../../lib/query';
import { AskButton } from '../../components/AskButton';
import { Button, Empty, ErrorLine, Spinner } from '../../components/ui';
import { t } from '../../i18n';
import { Header, Filter } from './parts';

/** Packages: a tab per package manager on the server; each tab loads (server-cached) on open. */
export function PackagesSection() {
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
