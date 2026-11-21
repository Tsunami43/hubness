import { useParams } from 'react-router';
import { NewSessionView, SessionView } from '../components/SessionView';
import { t } from '../i18n';
import { usePlace } from '../lib/places';
import { qk, useApi } from '../lib/query';
import { api } from '../lib/api';

export function ChatPage() {
  const { sid = 'new' } = useParams();
  // the chat's title for its tab in the top bar
  const { data: all } = useApi(qk.sessions('all'), () => api.allSessions(), { enabled: sid !== 'new' });
  const title = all?.find((s) => s.id === sid)?.title;
  usePlace(sid !== 'new' && all ? { key: `chat:${sid}`, kind: 'chat', title: title || t('psessions.new'), path: `/chats/${sid}` } : null);
  return sid === 'new' ? (
    <NewSessionView scope={null} backTo="/" basePath="/chats" title={t('chats.newTitle')} />
  ) : (
    <SessionView key={sid} sessionId={sid} backTo="/" />
  );
}
