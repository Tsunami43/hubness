import { useMemo, useState } from 'react';
import { Check, Search } from 'lucide-react';
import { getLang, LANGUAGES, setLang, t } from '../../i18n';

/** Name of a language in another language (`in`), or null when the browser cannot tell. */
function displayName(id: string, inLang: string): string | null {
  try {
    return new Intl.DisplayNames([inLang], { type: 'language' }).of(id) ?? null;
  } catch {
    return null;
  }
}

/** Settings → interface language, with search. Changing the language reloads the page. */
export function LanguagePage() {
  const current = getLang();
  const [q, setQ] = useState('');

  const items = useMemo(
    () =>
      LANGUAGES.map((l) => {
        // the name in the current UI language, shown next to the language's own name
        const local = displayName(l.id, current);
        const sub = local && local.toLocaleLowerCase() !== l.label.toLocaleLowerCase() ? local : null;
        const haystack = [l.id, l.label, local, displayName(l.id, 'en')].filter(Boolean).join(' ').toLocaleLowerCase();
        return { ...l, sub, haystack };
      }),
    [current],
  );
  const needle = q.trim().toLocaleLowerCase();
  const shown = needle ? items.filter((l) => l.haystack.includes(needle)) : items;

  return (
    <div className="space-y-6">
      <section>
        <h2 className="mb-2 font-semibold">{t('settings.language')}</h2>
        <div className="border border-line bg-surface">
          <label className="flex items-center gap-2 border-b border-line px-4">
            <Search className="size-4 shrink-0 text-muted" />
            <input
              type="search"
              value={q}
              onChange={(e) => setQ(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && shown.length === 1) setLang(shown[0].id);
              }}
              placeholder={t('settings.languageSearch')}
              aria-label={t('settings.languageSearch')}
              autoCapitalize="off"
              autoCorrect="off"
              spellCheck={false}
              className="min-w-0 flex-1 bg-transparent py-3 text-sm outline-none"
            />
          </label>
          {shown.length ? (
            <ul className="divide-y divide-line">
              {shown.map((l) => (
                <li key={l.id}>
                  <button type="button" lang={l.id} onClick={() => setLang(l.id)} className="flex w-full items-center gap-3 px-4 py-3 text-left hover:bg-sunken">
                    <span className="w-7 text-xs text-muted uppercase">{l.id}</span>
                    <span className="flex-1">
                      {l.label}
                      {l.sub ? <span className="ml-2 text-xs text-muted" lang={current}>{l.sub}</span> : null}
                    </span>
                    {l.id === current ? <Check className="size-4 text-accent" /> : null}
                  </button>
                </li>
              ))}
            </ul>
          ) : (
            <p className="px-4 py-3 text-sm text-muted">{t('settings.languageNone')}</p>
          )}
        </div>
        <p className="mt-2 text-xs text-muted">{t('settings.languageHint')}</p>
      </section>
    </div>
  );
}
