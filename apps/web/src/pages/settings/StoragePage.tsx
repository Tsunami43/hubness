import { useHubSettings } from '../../lib/hubSettings';
import { t } from '../../i18n';

/** Settings → storage: where the hub keeps its data and projects (read-only here; changed with hubness). */
export function StoragePage() {
  const s = useHubSettings();
  const rows: { label: string; value: string | undefined; hint: string; command: string }[] = [
    { label: t('storage.data'), value: s?.dataDir, hint: t('storage.dataHint'), command: 'hubness server config data-dir <path>' },
    { label: t('storage.projects'), value: s?.projectsRoot, hint: t('storage.projectsHint'), command: 'HUBNESS_HOME=<path> (hub.env)' },
  ];
  return (
    <div className="space-y-6">
      <section>
        <h2 className="mb-2 font-semibold">{t('storage.title')}</h2>
        <ul className="divide-y divide-line border border-line bg-surface">
          {rows.map((r) => (
            <li key={r.label} className="space-y-1 px-4 py-3">
              <div className="flex flex-wrap items-baseline gap-x-3">
                <span className="w-32 shrink-0 text-sm text-muted">{r.label}</span>
                <span className="font-mono text-sm break-all">{r.value ?? '…'}</span>
              </div>
              <p className="text-xs text-muted">
                {r.hint} <code className="font-mono">{r.command}</code>
              </p>
            </li>
          ))}
        </ul>
        <p className="mt-2 text-xs text-muted">{t('storage.restartHint')}</p>
      </section>
    </div>
  );
}
