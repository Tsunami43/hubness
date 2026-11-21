import { useEffect } from 'react';
import { installLeaderKeys, keyLabel, leaderLabel, useJumps, useKeyActionList, useKeyPrefs, useLeader } from '../lib/keys';
import { t } from '../i18n';

/**
 * Leader mode UI: installs the keyboard listener and, while leader mode is on, shows the
 * which-key panel (sections → key → action) at the bottom. With hints off — a small badge only.
 */
export function LeaderOverlay() {
  const active = useLeader();
  const prefs = useKeyPrefs();
  const actions = useKeyActionList();
  const jumps = useJumps();
  useEffect(() => installLeaderKeys(), []);
  if (!active) return null;
  // letters over the clickable items of the page
  const labels = prefs.hints
    ? jumps.map((j) => (
        <span
          key={j.key}
          className="pointer-events-none fixed z-[61] border border-accent bg-accent px-1 font-mono text-[11px] leading-4 font-semibold text-accent-fg shadow"
          // to the left of the item when there is room (the text stays readable), else over its start
          style={{
            left: j.rect.left > 26 ? j.rect.left - 4 : j.rect.left + 2,
            top: Math.max(2, j.rect.top + j.rect.height / 2 - 9),
            transform: j.rect.left > 26 ? 'translateX(-100%)' : undefined,
          }}
        >
          {j.key}
        </span>
      ))
    : null;

  if (!prefs.hints)
    return <div className="pointer-events-none fixed right-3 bottom-3 z-[60] border border-accent bg-surface px-2 py-1 text-xs text-accent">{leaderLabel(prefs.leader)} …</div>;
  // only page items to jump to (no screen actions besides navigation): a compact bar, the page stays visible
  const compact = jumps.length > 0 && actions.every((a) => a.key.length === 1 && (a.key !== a.key.toLowerCase() || !/[a-z0-9]/.test(a.key)));

  const sections = new Map<string, typeof actions>();
  for (const a of actions) sections.set(a.section, [...(sections.get(a.section) ?? []), a]);

  if (compact)
    return (
      <>
        {labels}
        <div className="pointer-events-none fixed inset-x-0 bottom-0 z-[60] flex justify-center p-3">
          <div className="flex max-w-4xl flex-wrap items-center gap-x-4 gap-y-1 border border-accent/60 bg-surface/95 px-3 py-2 text-xs shadow-2xl backdrop-blur">
            <span className="border border-accent px-1.5 text-accent">{leaderLabel(prefs.leader)}</span>
            <span className="text-muted">{t('keys.jumpHint')}</span>
            {actions.map((a) => (
              <span key={a.key} className="inline-flex items-center gap-1">
                <kbd className="border border-line bg-bg px-1 font-mono text-accent">{keyLabel(a.key)}</kbd>
                {a.label}
              </span>
            ))}
            <span className="text-muted">
              <kbd className="border border-line px-1">Esc</kbd> {t('keys.cancel')}
            </span>
          </div>
        </div>
      </>
    );

  return (
    <>
    {labels}
    <div className="pointer-events-none fixed inset-x-0 bottom-0 z-[60] flex justify-center p-3">
      <div className="pointer-events-auto max-h-[60dvh] w-full max-w-4xl overflow-auto border border-accent/60 bg-surface/95 p-3 shadow-2xl backdrop-blur">
        <div className="mb-2 flex items-center gap-2 text-xs text-muted">
          <span className="border border-accent px-1.5 text-accent">{leaderLabel(prefs.leader)}</span>
          <span className="flex-1">{t('keys.panelHint')}</span>
          <span>
            <kbd className="border border-line px-1">Esc</kbd> {t('keys.cancel')}
          </span>
        </div>
        <div className="grid gap-x-6 gap-y-3 sm:grid-cols-2 lg:grid-cols-3">
          {[...sections].map(([title, list]) => (
            <div key={title}>
              <div className="mb-1 text-[11px] font-semibold tracking-wider text-muted uppercase">{title}</div>
              <ul className="space-y-0.5">
                {list.map((a) => (
                  <li key={a.key} className="flex items-center gap-2 text-[13px]">
                    <kbd className="inline-flex min-w-7 justify-center border border-line bg-bg px-1 font-mono text-[12px] text-accent">{keyLabel(a.key)}</kbd>
                    <span className="truncate">{a.label}</span>
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>
        {jumps.length ? <div className="mt-2 text-xs text-muted">{t('keys.jumpHint')}</div> : null}
      </div>
    </div>
    </>
  );
}

/** Jump label shown over a tab / button while in leader mode (hints on). */
export function JumpLabel({ k, className = '' }: { k: string | undefined; className?: string }) {
  const active = useLeader();
  const { hints } = useKeyPrefs();
  if (!active || !hints || !k) return null;
  return (
    <span className={`pointer-events-none absolute z-10 border border-accent bg-accent px-1 font-mono text-[11px] leading-4 font-semibold text-accent-fg ${className || 'top-0.5 left-0.5'}`}>
      {keyLabel(k)}
    </span>
  );
}
