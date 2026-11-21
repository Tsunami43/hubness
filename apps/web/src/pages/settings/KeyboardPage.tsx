import { Check } from 'lucide-react';
import { keyLabel, leaderLabel, setKeyPrefs, useKeyPrefs, type LeaderKey } from '../../lib/keys';
import { Switch } from '../../components/ui';
import { t } from '../../i18n';

const LEADERS: LeaderKey[] = ['Control', 'Alt', 'Meta'];

/** Reference: what the leader opens (the panel itself lists what is available on the current screen). */
const REFERENCE: [string, [string, string][]][] = [
  ['keys.navigate', [['N', 'nav.new'], ['P', 'nav.projects'], ['S', 'nav.system'], ['A', 'nav.harness'], [',', 'nav.settings'], ['H', 'history.title']]],
  ['keys.tabs', [['1…9', 'keys.jumpTab'], ['h', 'keys.prevTab'], ['l', 'keys.nextTab'], ['w', 'tabs.close'], ['s', 'tabs.split'], ['d', 'tabs.splitDown'], ['[', 'keys.prevGroup'], [']', 'keys.nextGroup']]],
  ['keys.views', [['e', 'side.files'], ['f', 'side.search'], ['g', 'side.git'], ['p', 'keys.goToFile'], ['b', 'keys.sidebar']]],
  ['keys.tools', [['c', 'tab.sessions'], ['r', 'run.title'], ['v', 'preview.title'], ['t', 'shell.title'], ['k', 'secrets.title'], ['o', 'ws.settings']]],
];

const DIRECT: [string, string][] = [
  ['Ctrl+P', 'keys.goToFile'],
  ['Ctrl+Shift+F', 'side.search'],
  ['Ctrl+\\', 'tabs.split'],
  ['Ctrl+Shift+\\', 'tabs.splitDown'],
  ['Ctrl+B', 'keys.sidebar'],
  ['Alt+W', 'tabs.close'],
  ['Alt+1…6', 'keys.focusGroup'],
  ['Ctrl+S', 'common.save'],
];

/** Settings → Keyboard: leader key, hints, the list of shortcuts. Stored in this browser. */
export function KeyboardPage() {
  const prefs = useKeyPrefs();
  return (
    <div className="space-y-6">
      <section className="space-y-3">
        <h2 className="font-semibold">{t('keys.title')}</h2>
        <div className="space-y-3 border border-line bg-surface px-4 py-3">
          <Switch checked={prefs.enabled} onChange={(v) => setKeyPrefs({ enabled: v })} label={t('keys.enabled')} hint={t('keys.enabledHint', { key: leaderLabel(prefs.leader) })} />
          <Switch checked={prefs.hints} disabled={!prefs.enabled} onChange={(v) => setKeyPrefs({ hints: v })} label={t('keys.hints')} hint={t('keys.hintsHint')} />
        </div>
        <div>
          <div className="mb-1.5 text-sm font-medium">{t('keys.leader')}</div>
          <ul className="divide-y divide-line border border-line bg-surface">
            {LEADERS.map((l) => (
              <li key={l}>
                <button type="button" disabled={!prefs.enabled} onClick={() => setKeyPrefs({ leader: l })} className="flex w-full items-start gap-3 px-4 py-2.5 text-left hover:bg-sunken disabled:opacity-50">
                  <kbd className="mt-0.5 min-w-14 border border-line bg-bg px-1.5 text-center font-mono text-[12px]">{leaderLabel(l)}</kbd>
                  <span className="min-w-0 flex-1 text-xs text-muted">{t(`keys.leader.${l}`)}</span>
                  {prefs.leader === l ? <Check className="size-4 shrink-0 text-accent" /> : null}
                </button>
              </li>
            ))}
          </ul>
        </div>
      </section>

      <section>
        <h2 className="mb-2 font-semibold">{t('keys.reference', { key: leaderLabel(prefs.leader) })}</h2>
        <div className="grid gap-4 border border-line bg-surface p-4 sm:grid-cols-2">
          {REFERENCE.map(([title, list]) => (
            <div key={title}>
              <div className="mb-1 text-[11px] font-semibold tracking-wider text-muted uppercase">{t(title as 'keys.tabs')}</div>
              <ul className="space-y-0.5">
                {list.map(([k, label]) => (
                  <li key={k} className="flex items-center gap-2 text-[13px]">
                    <kbd className="inline-flex min-w-9 justify-center border border-line bg-bg px-1 font-mono text-[12px] text-accent">{k.length === 1 ? keyLabel(k) : k}</kbd>
                    {t(label as 'keys.tabs')}
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>
      </section>

      <section>
        <h2 className="mb-2 font-semibold">{t('keys.direct')}</h2>
        <ul className="space-y-0.5 border border-line bg-surface p-4">
          {DIRECT.map(([k, label]) => (
            <li key={k} className="flex items-center gap-2 text-[13px]">
              <kbd className="inline-flex min-w-28 justify-center border border-line bg-bg px-1 font-mono text-[12px]">{k}</kbd>
              {t(label as 'keys.tabs')}
            </li>
          ))}
        </ul>
      </section>
    </div>
  );
}
