import { Suspense, useState, type ReactNode } from 'react';
import { ChevronRight, Search, X } from 'lucide-react';
import { Navigate, NavLink, Outlet, useLocation, useNavigate } from 'react-router';
import { t } from '../i18n';
import { PageSpinner, TopBar } from './ui';

export interface SubmenuItem {
  to: string;
  label: string;
  hint?: string;
  icon: ReactNode;
  /** Words the section is found by in search (besides the title and hint). */
  keywords?: string[];
  /** Right side of the menu item: status, badge. */
  trailing?: ReactNode;
  /** A heading shown above the first item of a run of items with the same group. */
  group?: string;
}

const isDesktop = () => window.matchMedia('(min-width: 1024px)').matches;

const matches = (item: SubmenuItem, q: string) =>
  [item.label, item.hint ?? '', ...(item.keywords ?? [])].join(' ').toLowerCase().includes(q.trim().toLowerCase());

/**
 * Section with a submenu (Settings, Harness). Desktop: search and list on the left, content
 * on the right. Phone: base is the list, base/<item> is a page with "back".
 */
export function SubmenuLayout({
  base,
  title,
  subtitle,
  items,
  searchPlaceholder,
  actions,
  outletContext,
}: {
  base: string;
  title: string;
  subtitle?: ReactNode;
  items: SubmenuItem[];
  searchPlaceholder: string;
  actions?: ReactNode;
  outletContext?: unknown;
}) {
  const { pathname } = useLocation();
  const nav = useNavigate();
  const [q, setQ] = useState('');
  const current = items.find((i) => pathname === `${base}/${i.to}` || pathname.startsWith(`${base}/${i.to}/`));
  const isIndex = !current;
  const filtered = q.trim() ? items.filter((i) => matches(i, q)) : items;

  return (
    <div className="flex min-h-full flex-col">
      <div className="lg:hidden">
        <TopBar
          title={current ? current.label : title}
          subtitle={current ? undefined : subtitle}
          back={current ? base : undefined}
          actions={actions}
        />
      </div>
      <div className="hidden lg:block">
        <TopBar title={title} subtitle={subtitle} actions={actions} />
      </div>

      {/* Desktop: three columns, content in the center (aligned with the header title),
          the menu right to its left, the right column empty for symmetry */}
      <div className="w-full flex-1 lg:grid lg:grid-cols-[minmax(15rem,1fr)_minmax(0,48rem)_minmax(0,1fr)] lg:gap-6 lg:px-6 lg:py-6">
        <nav className={`${isIndex ? 'block' : 'hidden'} w-full p-4 lg:block lg:w-60 lg:justify-self-end lg:p-0`}>
          <div className="relative mb-3">
            <Search className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted" />
            <input
              type="search"
              value={q}
              onChange={(e) => setQ(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && filtered[0]) nav(filtered[0].to.startsWith('/') ? filtered[0].to : `${base}/${filtered[0].to}`);
                if (e.key === 'Escape') setQ('');
              }}
              placeholder={searchPlaceholder}
              aria-label={searchPlaceholder}
              className="w-full rounded-xl border border-line bg-surface py-2 pr-9 pl-9 text-sm outline-none placeholder:text-muted/70 focus:border-accent [&::-webkit-search-cancel-button]:hidden"
            />
            {q ? (
              <button
                type="button"
                onClick={() => setQ('')}
                aria-label={t('common.clear')}
                className="absolute top-1/2 right-2 inline-flex size-6 -translate-y-1/2 items-center justify-center rounded-md text-muted hover:text-fg"
              >
                <X className="size-3.5" />
              </button>
            ) : null}
          </div>

          {filtered.length ? (
            <ul className="space-y-1">
              {filtered.map((item, i) => (
                <li key={item.to}>
                  {item.group && item.group !== filtered[i - 1]?.group ? (
                    <div className={`px-3 pb-1 text-[11px] font-medium tracking-wide text-muted uppercase ${i ? 'pt-3' : ''}`}>{item.group}</div>
                  ) : null}
                  <NavLink
                    data-jump
                    to={item.to}
                    className={({ isActive }) => `flex items-center gap-3 rounded-xl px-3 py-2.5 ${isActive ? 'bg-sunken' : 'hover:bg-sunken'}`}
                  >
                    <span className="flex size-5 shrink-0 items-center justify-center">{item.icon}</span>
                    <span className="min-w-0 flex-1">
                      <span className="block truncate font-medium">{item.label}</span>
                      {item.hint ? <span className="block truncate text-xs text-muted">{item.hint}</span> : null}
                    </span>
                    {item.trailing}
                    <ChevronRight className="size-4 shrink-0 text-muted lg:hidden" />
                  </NavLink>
                </li>
              ))}
            </ul>
          ) : (
            <p className="px-3 py-2 text-sm text-muted">{t('search.noResults', { q: q.trim() })}</p>
          )}
        </nav>

        <div className={`${isIndex ? 'hidden' : 'block'} min-w-0 p-4 lg:block lg:p-0`}>
          <Suspense fallback={<PageSpinner />}>
            <Outlet context={outletContext} />
          </Suspense>
        </div>
      </div>
    </div>
  );
}

/** Section index: on desktop the first item opens right away, on phone the list stays. */
export function SubmenuIndex({ to }: { to: string | undefined }) {
  return isDesktop() && to ? <Navigate to={to} replace /> : null;
}
