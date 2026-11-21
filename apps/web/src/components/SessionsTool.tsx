import { useEffect, useState } from 'react';
import { useSearchParams } from 'react-router';
import { MessageSquarePlus, Plus } from 'lucide-react';
import { useSessions, type SessionScope } from '../lib/hooks';
import { lastSessionKey } from '../lib/ask';
import { t } from '../i18n';
import { NewSessionView, SessionView } from './SessionView';
import { SessionList } from './SessionList';
import { Button, Empty, Spinner } from './ui';
import { useWorkbench } from '../lib/workbench';
import { agentLabel } from '../lib/format';
import { matchesQuery, SessionSearch } from './SessionSearch';

/**
 * Sessions tab of a project/group: the list. Inside a workbench every session opens as
 * its own tab (split them, put one next to code); outside — right in the panel (?s=).
 */
export function SessionsTool({ scope, root, title }: { scope: SessionScope; root?: string; title: string }) {
  const [params, setParams] = useSearchParams();
  const wb = useWorkbench();
  // in a workbench ?s= belongs to the session tab, not to this list
  const sid = wb ? null : params.get('s');
  const { sessions, loading } = useSessions(scope);
  const [query, setQuery] = useState('');
  const shown = sessions?.filter((s) => matchesQuery(query, s.title, agentLabel(s.agent)));
  // scope key, same as the workspace storageKey ("ask the agent" drafts)
  const scopeKey = 'projectId' in scope ? scope.projectId : `group:${scope.groupId}`;
  const set = (id: string | null) => {
    // in a workbench the list tab turns into the session (no list left behind)
    if (wb && id) return wb.openSession(id, { replace: 't:sessions' });
    if (id && id !== 'new') {
      try {
        localStorage.setItem(lastSessionKey(scopeKey), JSON.stringify(id));
      } catch {}
    }
    const next = new URLSearchParams(params);
    if (id) next.set('s', id);
    else next.delete('s');
    setParams(next, { replace: true });
  };
  useEffect(() => {
    if (sid && sid !== 'new') {
      try {
        localStorage.setItem(lastSessionKey(scopeKey), JSON.stringify(sid));
      } catch {}
    }
  }, [sid, scopeKey]);
  const base = 'projectId' in scope ? `/projects/${scope.projectId}/s` : `/groups/${scope.groupId}/s`;

  if (sid === 'new')
    return <NewSessionView scope={scope} backTo="" basePath={base} title={t('session.newIn', { project: title })} onBack={() => set(null)} onCreated={set} draftKey={`${scopeKey}:new`} />;
  if (sid) return <SessionView key={sid} sessionId={sid} backTo="" root={root} onBack={() => set(null)} draftKey={`${scopeKey}:${sid}`} />;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-2 border-b border-line px-3 py-1.5">
        <span className="min-w-0 flex-1 truncate text-xs text-muted">{'groupId' in scope ? t('gsessions.hint') : t('psessions.hint')}</span>
        <Button size="sm" variant="primary" onClick={() => set('new')}>
          <Plus className="size-3.5" /> {t('psessions.new')}
        </Button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        {loading && !sessions ? (
          <div className="flex justify-center py-10">
            <Spinner />
          </div>
        ) : sessions?.length ? (
          <>
            <div className="px-3 pt-2">
              <SessionSearch value={query} onChange={setQuery} />
            </div>
            {shown?.length ? <SessionList sessions={shown} onPick={(s) => set(s.id)} /> : <div className="px-3 py-3 text-sm text-muted">{t('sessions.nothingFound')}</div>}
          </>
        ) : (
          <Empty
            icon={<MessageSquarePlus className="size-8" />}
            title={t('psessions.empty')}
            hint={'groupId' in scope ? t('gsessions.emptyHint') : t('psessions.emptyHint')}
            action={
              <Button variant="primary" onClick={() => set('new')}>
                {t('psessions.newSession')}
              </Button>
            }
          />
        )}
      </div>
    </div>
  );
}
