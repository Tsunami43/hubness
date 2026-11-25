import { useEffect, useState } from 'react';
import { Navigate, useParams } from 'react-router';
import { ErrorLine, Spinner } from '../components/ui';
import { sessionPath } from '../components/Layout';
import { api } from '../lib/api';

/** /session/:id — open a session without knowing whether it is a chat or a project session. */
export function SessionRedirect() {
  const { id = '' } = useParams();
  const [to, setTo] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    api.session(id).then(
      (s) => setTo(sessionPath(s)),
      (e: Error) => setError(e.message),
    );
  }, [id]);
  if (error) return <div className="p-4"><ErrorLine error={error} /></div>;
  return to ? <Navigate to={to} replace /> : <div className="flex justify-center py-10"><Spinner /></div>;
}
