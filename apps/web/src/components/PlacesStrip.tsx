import { Boxes, FolderGit2, MessageSquare, X } from 'lucide-react';
import { Link, useLocation, useNavigate } from 'react-router';
import { closePlace, usePlaces, type PlaceKind } from '../lib/places';
import { t } from '../i18n';

const ICON: Record<PlaceKind, typeof FolderGit2> = { project: FolderGit2, group: Boxes, chat: MessageSquare };

/** Open projects / groups / chats as tabs in the top bar (desktop). */
export function PlacesStrip() {
  const places = usePlaces();
  const loc = useLocation();
  const nav = useNavigate();
  if (!places.length) return null;
  return (
    <nav aria-label={t('places.label')} className="hidden min-w-0 shrink items-stretch self-stretch overflow-x-auto [scrollbar-width:none] lg:flex">
      {places.map((p) => {
        const active = loc.pathname === p.path || loc.pathname.startsWith(`${p.path}/`);
        const Icon = ICON[p.kind];
        return (
          <div
            key={p.key}
            className={`group relative flex max-w-52 shrink-0 items-center border-b-2 ${active ? 'border-accent bg-bg text-fg' : 'border-transparent text-muted hover:bg-sunken hover:text-fg'}`}
          >
            <Link to={active ? loc.pathname + loc.search : p.url} title={p.title} className="flex min-w-0 items-center gap-1.5 py-1 pr-1 pl-2.5 text-[13px]">
              <Icon className={`size-3.5 shrink-0 ${active ? 'text-accent' : ''}`} />
              <span className="truncate">{p.title}</span>
            </Link>
            <button
              type="button"
              aria-label={t('places.close', { name: p.title })}
              title={t('places.close', { name: p.title })}
              onClick={() => {
                const next = closePlace(p.key);
                if (active) nav(next ? next.url : '/');
              }}
              className={`mr-1 inline-flex size-5 shrink-0 items-center justify-center hover:bg-sunken hover:text-fg ${active ? '' : 'opacity-0 group-hover:opacity-100'}`}
            >
              <X className="size-3" />
            </button>
          </div>
        );
      })}
    </nav>
  );
}
