import { Check } from 'lucide-react';
import { getLang, LANGUAGES, setLang } from '../../i18n';
import { t } from '../../i18n';

/** Settings → interface language. Changing the language reloads the page. */
export function LanguagePage() {
  const current = getLang();
  return (
    <div className="space-y-6">
      <section>
        <h2 className="mb-2 font-semibold">{t('settings.language')}</h2>
        <ul className="divide-y divide-line border border-line bg-surface">
          {LANGUAGES.map((l) => (
            <li key={l.id}>
              <button type="button" onClick={() => setLang(l.id)} className="flex w-full items-center gap-3 px-4 py-3 text-left hover:bg-sunken">
                <span className="w-7 text-xs text-muted uppercase">{l.id}</span>
                <span className="flex-1">{l.label}</span>
                {l.id === current ? <Check className="size-4 text-accent" /> : null}
              </button>
            </li>
          ))}
        </ul>
        <p className="mt-2 text-xs text-muted">{t('settings.languageHint')}</p>
      </section>
    </div>
  );
}
