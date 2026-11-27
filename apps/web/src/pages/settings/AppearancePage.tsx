import { useSyncExternalStore } from 'react';
import { Check, Minus, PanelLeft, PanelRight, Plus } from 'lucide-react';
import { t } from '../../i18n';
import { currentThemes, getStyleId, getThemeId, isDark, onThemeChange, setStyle, setTheme, STYLES, type StyleId } from '../../theme';
import { CODE_SIZE, setCodeSize, setSideRight, useSideRight, setRelativeLines, setTabSize, setVimMode, TAB_SIZES, useCodeSize, useRelativeLines, useTabSize, useVimMode } from '../../lib/editorPrefs';
import { IconButton, Switch } from '../../components/ui';

/** Appearance: theme picker (each theme has dark and light variants; swatches show the current mode) and editor options. */
export function AppearancePage() {
  const themeId = useSyncExternalStore(onThemeChange, getThemeId);
  const styleId = useSyncExternalStore(onThemeChange, getStyleId);
  const vim = useVimMode();
  const size = useCodeSize();
  const tab = useTabSize();
  const relative = useRelativeLines();
  const sideRight = useSideRight();

  return (
    <div className="space-y-6">
      <section>
        <h2 className="mb-2 font-semibold">{t('settings.style')}</h2>
        <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
          {STYLES.map((st) => (
            <button
              key={st.id}
              type="button"
              onClick={() => setStyle(st.id)}
              aria-pressed={st.id === styleId}
              className={`flex flex-col gap-2 rounded-xl border bg-surface p-3 text-left ${st.id === styleId ? 'border-accent' : 'border-line hover:border-muted'}`}
            >
              <StylePreview id={st.id} />
              <span className="flex items-center gap-1.5 text-sm font-medium">
                {t(`style.${st.id}` as const)}
                {st.id === styleId ? <Check className="size-4 text-accent" /> : null}
              </span>
              <span className="text-xs text-muted">{t(`style.${st.id}Hint` as const)}</span>
            </button>
          ))}
        </div>
      </section>
      <section>
        <h2 className="mb-2 font-semibold">{t('settings.theme')}</h2>
        <ul className="divide-y divide-line overflow-hidden rounded-xl border border-line bg-surface">
          {currentThemes().map((th) => {
            const p = isDark() ? th.dark : th.light;
            return (
              <li key={th.id}>
                <button type="button" onClick={() => setTheme(th.id)} className="flex w-full items-center gap-3 px-4 py-3 text-left hover:bg-sunken">
                  <span className="flex overflow-hidden rounded-md border border-line" aria-hidden>
                    {[p.bg, p.panel, p.fg, p.accent, p.syntax.keyword, p.syntax.function, p.syntax.string].map((c, i) => (
                      <span key={i} className="size-4" style={{ background: c }} />
                    ))}
                  </span>
                  <span className="flex-1 font-medium">{th.label}</span>
                  {th.id === themeId ? <Check className="size-5 text-accent" /> : null}
                </button>
              </li>
            );
          })}
        </ul>
      </section>
      <section>
        <h2 className="mb-2 font-semibold">{t('settings.workspace')}</h2>
        <div className="rounded-xl border border-line bg-surface px-4 py-3">
          <div className="flex flex-wrap items-center gap-3">
            <div className="min-w-0 flex-1">
              <div>{t('settings.sideBar')}</div>
              <div className="text-xs text-muted">{t('settings.sideBarHint')}</div>
            </div>
            <div className="flex border border-line" role="radiogroup" aria-label={t('settings.sideBar')}>
              {([false, true] as const).map((right) => (
                <button
                  key={String(right)}
                  type="button"
                  role="radio"
                  aria-checked={sideRight === right}
                  onClick={() => setSideRight(right)}
                  className={`flex h-8 items-center gap-1.5 px-3 text-sm ${sideRight === right ? 'bg-sunken text-accent' : 'text-muted hover:text-fg'}`}
                >
                  {right ? <PanelRight className="size-4" /> : <PanelLeft className="size-4" />}
                  {right ? t('settings.sideRight') : t('settings.sideLeft')}
                </button>
              ))}
            </div>
          </div>
        </div>
      </section>
      <section>
        <h2 className="mb-2 font-semibold">{t('settings.editor')}</h2>
        <div className="space-y-3 rounded-xl border border-line bg-surface px-4 py-3">
          <Switch checked={vim} onChange={setVimMode} label={t('settings.vim')} hint={t('settings.vimHint')} />
          <Switch checked={relative} onChange={setRelativeLines} label={t('settings.relativeLines')} hint={t('settings.relativeLinesHint')} />
          <div className="flex flex-wrap items-center gap-3">
            <div className="min-w-0 flex-1">
              <div>{t('settings.tabSize')}</div>
              <div className="text-xs text-muted">{t('settings.tabSizeHint')}</div>
            </div>
            <div className="flex border border-line" role="radiogroup" aria-label={t('settings.tabSize')}>
              {TAB_SIZES.map((n) => (
                <button
                  key={n}
                  type="button"
                  role="radio"
                  aria-checked={tab === n}
                  onClick={() => setTabSize(n)}
                  className={`h-8 w-11 font-mono text-sm ${tab === n ? 'bg-sunken text-accent' : 'text-muted hover:text-fg'}`}
                >
                  {n}
                </button>
              ))}
            </div>
          </div>
          <div className="flex flex-wrap items-center gap-3">
            <div className="min-w-0 flex-1">
              <div>{t('settings.codeSize')}</div>
              <div className="text-xs text-muted">{t('settings.codeSizeHint')}</div>
            </div>
            <div className="flex items-center border border-line">
              <IconButton label={t('settings.codeSmaller')} onClick={() => setCodeSize(size - 1)} disabled={size <= CODE_SIZE.min} className="size-8">
                <Minus className="size-4" />
              </IconButton>
              <span className="w-14 text-center font-mono text-sm tabular-nums">{size} px</span>
              <IconButton label={t('settings.codeLarger')} onClick={() => setCodeSize(size + 1)} disabled={size >= CODE_SIZE.max} className="size-8">
                <Plus className="size-4" />
              </IconButton>
            </div>
          </div>
          <pre className="overflow-x-auto border border-line bg-bg px-3 py-2 font-mono text-[length:var(--code-size,12px)] leading-[1.6] text-fg">
            {'def main():\n    print("hello world")  # 0O 1lI'}
          </pre>
          {size !== CODE_SIZE.default ? (
            <button type="button" className="text-xs text-muted hover:text-fg hover:underline" onClick={() => setCodeSize(CODE_SIZE.default)}>
              {t('settings.codeSizeReset', { n: CODE_SIZE.default })}
            </button>
          ) : null}
        </div>
      </section>
    </div>
  );
}

/** A tiny drawing of a style's shapes (the real style applies to the whole app when chosen). */
function StylePreview({ id }: { id: StyleId }) {
  const box: Record<StyleId, string> = {
    strict: 'rounded-none border border-line font-mono',
    glass: 'rounded-[12px] border border-white/20 bg-white/10 shadow-lg backdrop-blur',
    expressive: 'rounded-[18px] border border-transparent bg-sunken',
    brutal: 'rounded-none border-2 border-fg shadow-[3px_3px_0_0_currentColor]',
  };
  const btn: Record<StyleId, string> = {
    strict: 'rounded-none',
    glass: 'rounded-[8px]',
    expressive: 'rounded-full',
    brutal: 'rounded-none border-2 border-fg',
  };
  const backdrop = id === 'glass' ? 'bg-[radial-gradient(circle_at_20%_20%,#8b7dff66,transparent_60%),radial-gradient(circle_at_80%_70%,#22d3ee55,transparent_60%)]' : '';
  return (
    <span aria-hidden className={`flex h-14 w-full items-center justify-center rounded-md ${backdrop}`}>
      <span className={`flex w-24 items-center justify-between gap-1 px-2 py-1.5 ${box[id]}`}>
        <span className="h-1.5 w-8 rounded-full bg-muted/60" />
        <span className={`h-4 w-7 bg-accent ${btn[id]}`} />
      </span>
    </span>
  );
}
