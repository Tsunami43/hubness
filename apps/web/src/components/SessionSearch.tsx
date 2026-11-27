import { forwardRef } from 'react';
import { Search, X } from 'lucide-react';
import { t } from '../i18n';

/** Case-insensitive match of every word of the query against any of the fields. */
export function matchesQuery(query: string, ...fields: (string | null | undefined)[]): boolean {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return true;
  const hay = fields.filter(Boolean).join(' ').toLowerCase();
  return words.every((w) => hay.includes(w));
}

/** Search box above a session list: filters as you type, Esc clears. */
export const SessionSearch = forwardRef<HTMLInputElement, { value: string; onChange: (v: string) => void; className?: string }>(function SessionSearch(
  { value, onChange, className = '' },
  ref,
) {
  return (
    <div className={`flex items-center gap-2 border border-line bg-sunken px-2 focus-within:border-accent ${className}`}>
      <Search className="size-3.5 shrink-0 text-muted" />
      <input
        ref={ref}
        type="search"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Escape' && value) {
            e.stopPropagation();
            onChange('');
          }
        }}
        placeholder={t('sessions.search')}
        aria-label={t('sessions.search')}
        className="min-w-0 flex-1 bg-transparent py-1.5 text-sm outline-none pointer-coarse:py-2.5 pointer-coarse:text-[15px] placeholder:text-muted/70 [&::-webkit-search-cancel-button]:hidden"
      />
      {value ? (
        <button type="button" onClick={() => onChange('')} aria-label={t('sessions.searchClear')} title={t('sessions.searchClear')} className="tap -mr-2 inline-flex items-center justify-center text-muted hover:text-fg">
          <X className="size-3.5" />
        </button>
      ) : null}
    </div>
  );
});
