import type { RepoLanguage } from '@hubness/shared';
import { t } from '../i18n';

/**
 * Repository languages, as on GitHub: a bar by share and labels. Colors are
 * GitHub Linguist data from the server (not the theme): they make a language recognizable at a glance.
 */
export function LanguageBar({ languages }: { languages: RepoLanguage[] }) {
  if (!languages.length) return null;
  const shown = languages.slice(0, 7);
  const other = languages.slice(7).reduce((a, l) => a + l.percent, 0);
  return (
    <div>
      <div className="flex h-2 overflow-hidden rounded-full bg-sunken">
        {shown.map((l) => (
          <span key={l.name} title={`${l.name} ${l.percent}%`} style={{ width: `${l.percent}%`, backgroundColor: l.color ?? undefined }} className={l.color ? '' : 'bg-muted'} />
        ))}
      </div>
      <ul className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-xs">
        {shown.map((l) => (
          <li key={l.name} className="inline-flex items-center gap-1.5">
            <LanguageDot color={l.color} />
            <span className="font-medium">{l.name}</span>
            <span className="text-muted">{l.percent}%</span>
          </li>
        ))}
        {other > 0 ? (
          <li className="inline-flex items-center gap-1.5">
            <LanguageDot color={null} />
            <span className="font-medium">{t('languages.other')}</span>
            <span className="text-muted">{Math.round(other * 10) / 10}%</span>
          </li>
        ) : null}
      </ul>
    </div>
  );
}

export function LanguageDot({ color }: { color: string | null }) {
  return <span className={`size-2.5 shrink-0 rounded-full ${color ? '' : 'bg-muted'}`} style={color ? { backgroundColor: color } : undefined} />;
}
