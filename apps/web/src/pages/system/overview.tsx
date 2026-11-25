import { useEffect, type ReactNode } from 'react';
import { Link } from 'react-router';
import type { HostInfo, SystemOverview } from '@hubness/shared';
import { api } from '../../lib/api';
import { useApi } from '../../lib/query';
import { OsLogo } from '../../components/OsLogo';
import { ErrorLine, Spinner } from '../../components/ui';
import { t } from '../../i18n';
import { fmtUptime, Header, Bar } from './parts';

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

export function OverviewSection() {
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
