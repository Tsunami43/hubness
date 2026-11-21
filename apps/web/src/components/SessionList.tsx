import { Link } from 'react-router';
import type { Session } from '@hubness/shared';
import { agentLabel, timeAgo } from '../lib/format';
import { StatusBadge } from './ui';
import { AgentIcon } from './AgentIcon';

export function SessionList({ sessions, base, onPick }: { sessions: Session[]; base?: string; onPick?: (s: Session) => void }) {
  return (
    <ul className="divide-y divide-line">
      {sessions.map((s) => (
        <li key={s.id}>
          <Link
            data-jump
            to={onPick ? '#' : `${base}/${s.id}`}
            onClick={
              onPick
                ? (e) => {
                    e.preventDefault();
                    onPick(s);
                  }
                : undefined
            }
            className="flex items-center gap-3 px-3 py-2.5 hover:bg-sunken active:bg-sunken"
          >
            <AgentIcon kind={s.agent.kind} className="size-[18px]" />
            <div className="min-w-0 flex-1">
              <div className="truncate">{s.title}</div>
              <div className="flex items-center gap-2 truncate text-xs text-muted">
                <span className="truncate">{agentLabel(s.agent)}</span>
              </div>
            </div>
            <div className="flex shrink-0 flex-col items-end gap-0.5">
              <span className="text-xs text-muted">{timeAgo(s.updatedAt)}</span>
              <StatusBadge status={s.status} />
            </div>
          </Link>
        </li>
      ))}
    </ul>
  );
}
