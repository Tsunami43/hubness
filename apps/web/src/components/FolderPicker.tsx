import { useEffect, useState } from 'react';
import { ChevronUp, Folder, GitBranch } from 'lucide-react';
import type { BrowseResponse } from '@hubness/shared';
import { api } from '../lib/api';
import { ErrorLine, inputCls, Spinner } from './ui';
import { t } from '../i18n';

/** Browse folders on the server. The current folder is the selected one. */
export function FolderPicker({ onPick }: { onPick: (path: string, isGit: boolean) => void }) {
  const [data, setData] = useState<BrowseResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [manual, setManual] = useState('');

  const open = async (path?: string) => {
    try {
      const d = await api.browse(path);
      setData(d);
      setManual(d.path);
      setError(null);
      onPick(d.path, d.isGitRepo);
    } catch (e) {
      setError((e as Error).message);
    }
  };

  useEffect(() => {
    void open();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div className="space-y-2">
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void open(manual);
        }}
      >
        <input
          className={`${inputCls} font-mono text-sm`}
          value={manual}
          onChange={(e) => setManual(e.target.value)}
          placeholder="/home/me/projects/app"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
        />
      </form>
      <ErrorLine error={error} />
      {!data ? (
        <Spinner />
      ) : (
        <div className="overflow-hidden rounded-lg border border-line">
          {data.isGitRepo ? (
            <div className="flex items-center gap-2 border-b border-line bg-ok/10 px-3 py-2 text-sm text-ok">
              <GitBranch className="size-4" /> {t('folder.isRepo')}
            </div>
          ) : null}
          <ul className="max-h-72 divide-y divide-line overflow-y-auto">
            {data.parent ? (
              <li>
                <button type="button" onClick={() => open(data.parent!)} className="flex w-full items-center gap-2.5 px-3 py-2.5 text-left hover:bg-sunken">
                  <ChevronUp className="size-4 text-muted" /> <span className="text-muted">{t('folder.up')}</span>
                </button>
              </li>
            ) : null}
            {data.entries.map((e) => (
              <li key={e.path}>
                <button type="button" onClick={() => open(e.path)} className="flex w-full items-center gap-2.5 px-3 py-2.5 text-left hover:bg-sunken">
                  <Folder className="size-4 text-muted" /> <span className="truncate">{e.name}</span>
                </button>
              </li>
            ))}
            {data.entries.length === 0 ? <li className="px-3 py-3 text-sm text-muted">{t('folder.noSubfolders')}</li> : null}
          </ul>
        </div>
      )}
    </div>
  );
}
