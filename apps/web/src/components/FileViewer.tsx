import { lazy, Suspense, useCallback, useEffect, useRef, useState } from 'react';
import { ChevronLeft, ChevronRight } from 'lucide-react';
import type { GitStatus } from '@hubness/shared';
import { api, ApiError } from '../lib/api';
import { useRelativeLines, useTabSize, useVimMode } from '../lib/editorPrefs';
import { useApi } from '../lib/query';
import { touches, useFileChanges } from '../lib/fileWatch';
import { fmtSize } from '../lib/format';
import { t } from '../i18n';
import { DiffView } from './DiffView';
import { Markdown } from './Markdown';
import { CodeLine, useTokens } from './Code';
import { FileIcon } from './FileIcon';
import { codeSnippet, SelectionAsk } from './SelectionAsk';
import { Button, Empty, ErrorLine, Spinner } from './ui';

type Mode = 'file' | 'preview' | 'diff';
const isMarkdown = (p: string) => /\.(md|markdown|mdx)$/i.test(p);
const coarsePointer = () => typeof matchMedia === 'function' && matchMedia('(pointer: coarse)').matches;

// the editor (CodeMirror) is a separate chunk: loaded on the first Edit
const CodeEditor = lazy(() => import('./CodeEditor'));

/** Unsaved edits per file (in memory): switching files doesn't lose them. */
const drafts = new Map<string, { text: string; base: string }>();

/**
 * Save a file's unsaved draft without its viewer (closing a tab). null — saved (or nothing to
 * save); otherwise the error to show. A file changed on disk since the draft began is not
 * overwritten: the user decides in the tab (load or overwrite).
 */
export async function saveDraft(projectId: string, path: string): Promise<string | null> {
  const key = `${projectId}:${path}`;
  const d = drafts.get(key);
  if (!d) return null;
  try {
    await api.saveFile(projectId, { path, content: d.text, baseHash: d.base || undefined });
    drafts.delete(key);
    return null;
  } catch (e) {
    return e instanceof ApiError && e.status === 409 ? t('files.closeConflict') : (e as Error).message;
  }
}

/** Drop a file's unsaved draft (closing a tab without saving). */
export const discardDraft = (projectId: string, path: string) => void drafts.delete(`${projectId}:${path}`);

/** Project file viewer: code with line numbers, Markdown preview, diff of changes. */
export function FileViewer({
  projectId,
  rootName,
  path,
  git,
  onBack,
  onCrumb,
  line,
  nonce,
  initialMode,
  onDirty,
  onPin,
}: {
  /** Project = the repository the file comes from. */
  projectId: string;
  /** Repository name in the breadcrumbs (in a group). */
  rootName?: string;
  path: string;
  git: GitStatus | null;
  /** Phone: back to the tree. */
  onBack?: () => void;
  /** Click on a folder in the breadcrumbs. */
  onCrumb?: (dir: string) => void;
  /** Line to reveal (search result); `nonce` changes on every request. */
  line?: number;
  nonce?: number;
  /** Open in diff mode (from the Git view). */
  initialMode?: 'diff';
  /** Unsaved edits appeared / were saved or dropped (tab dot). */
  onDirty?: (dirty: boolean) => void;
  /** The user started working with the file: a preview tab becomes a regular one. */
  onPin?: () => void;
}) {
  const { data, setData, loading, error } = useApi(['file', projectId, path], () => api.file(projectId, path));
  const draftKey = `${projectId}:${path}`;
  const vimMode = useVimMode();
  const tabSize = useTabSize();
  const relativeLines = useRelativeLines();
  // opened straight in the editor (from the tree, not by typing): don't pin the tab
  const [autoEdit, setAutoEdit] = useState(false);
  const [editing, setEditing] = useState(() => drafts.has(draftKey));
  const [restored, setRestored] = useState(() => drafts.has(draftKey));
  const [draft, setDraft] = useState<string | null>(() => drafts.get(draftKey)?.text ?? null);
  const [editorKey, setEditorKey] = useState(0);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [conflict, setConflict] = useState(false);
  const [confirmDiscard, setConfirmDiscard] = useState(false);
  const [savedAt, setSavedAt] = useState(0);
  // the file changed on disk: editor text replaced from outside / deleted / diff to reload
  const [external, setExternal] = useState<{ text: string; rev: number }>();
  const [gone, setGone] = useState(false);
  const [diffRev, setDiffRev] = useState(0);
  const editable = Boolean(data && !data.binary && !data.truncated && data.hash);
  const dirty = editing && draft !== null && draft !== data?.content;
  // the draft was started on an older version of the file (an agent changed it since):
  // never pass it off as the current file — the conflict bar offers to load or overwrite
  const draftBase = dirty ? drafts.get(draftKey)?.base : undefined;
  const stale = Boolean(draftBase && data?.hash && draftBase !== data.hash);
  const dirtyCb = useRef(onDirty);
  dirtyCb.current = onDirty;
  useEffect(() => dirtyCb.current?.(dirty), [dirty]);
  useEffect(() => () => dirtyCb.current?.(false), []);
  const gitFile = git?.files.find((f) => f.path === path);
  const defaultMode: Mode = initialMode === 'diff' ? 'diff' : line ? 'file' : isMarkdown(path) ? 'preview' : 'file';
  const [mode, setMode] = useState<Mode>(defaultMode);
  useEffect(() => setMode(defaultMode), [path]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (initialMode === 'diff') setMode('diff');
  }, [initialMode]);
  // a text file is always open in the editor in File mode (no separate Edit step); Markdown
  // starts in its preview and diffs stay diffs until File is picked. After Cancel this
  // reopens the editor with the version from disk.
  useEffect(() => {
    if (editing || mode !== 'file' || !editable || !data) return;
    setRestored(drafts.has(draftKey));
    setDraft(drafts.get(draftKey)?.text ?? data.content);
    setAutoEdit(true);
    setEditing(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data?.hash, mode, editing, editable]);
  // only staged changes → show the staged diff (the worktree one is empty)
  const stagedOnly = Boolean(gitFile && gitFile.worktree === ' ' && gitFile.index !== ' ' && !gitFile.untracked);
  const diff = useApi(['git', projectId, 'diff', path, stagedOnly, Boolean(gitFile?.untracked), diffRev], () => api.gitDiff(projectId, { path, untracked: gitFile!.untracked, staged: stagedOnly }), {
    enabled: mode === 'diff' && Boolean(gitFile),
  });
  const text = data && !data.binary ? data.content.replace(/\n$/, '') : '';
  const lines = text ? text.split('\n') : [];
  const tokens = useTokens(text, mode === 'file' ? { path } : null);
  const parts = path.split('/');
  const codeRef = useRef<HTMLDivElement>(null);
  // selection → agent: path, line numbers and the fragment itself
  const formatSel = useCallback(
    (text: string, sel: Selection) => {
      const line = (n: Node | null) => Number((n instanceof Element ? n : n?.parentElement)?.closest('tr[data-line]')?.getAttribute('data-line') ?? 0);
      const a = line(sel.anchorNode);
      const b = line(sel.focusNode);
      const [from, to] = [Math.min(a, b) || 1, Math.max(a, b) || 1];
      return codeSnippet(path, from, to, text);
    },
    [path],
  );
  // reveal a line (search result): scroll to it and flash it
  const [flash, setFlash] = useState<number | null>(null);
  useEffect(() => {
    if (!line || !data || editing) return;
    if (mode !== 'file') setMode('file');
    const id = requestAnimationFrame(() => {
      codeRef.current?.querySelector(`tr[data-line="${line}"]`)?.scrollIntoView({ block: 'center' });
      setFlash(line);
    });
    const timer = setTimeout(() => setFlash(null), 1600);
    return () => {
      cancelAnimationFrame(id);
      clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [line, nonce, Boolean(data), editing, mode === 'file']);

  const modes: Mode[] = [...(isMarkdown(path) ? (['preview'] as Mode[]) : []), 'file', ...(gitFile ? (['diff'] as Mode[]) : [])];

  // closing the tab with unsaved edits asks the browser to confirm
  useEffect(() => {
    if (!dirty) return;
    const stop = (e: BeforeUnloadEvent) => e.preventDefault();
    window.addEventListener('beforeunload', stop);
    return () => window.removeEventListener('beforeunload', stop);
  }, [dirty]);

  const change = (text: string) => {
    if (autoEdit && text !== data?.content) onPin?.();
    setDraft(text);
    setConfirmDiscard(false);
    const base = drafts.get(draftKey)?.base ?? data?.hash ?? '';
    if (text === data?.content) drafts.delete(draftKey);
    else drafts.set(draftKey, { text, base });
  };
  const stopEdit = () => {
    drafts.delete(draftKey);
    setEditing(false);
    setDraft(null);
    setConflict(false);
    setSaveError(null);
    setConfirmDiscard(false);
  };
  const save = async (force = false) => {
    if (draft === null || saving) return;
    setSaving(true);
    setSaveError(null);
    try {
      const base = drafts.get(draftKey)?.base ?? data?.hash;
      const file = await api.saveFile(projectId, { path, content: draft, baseHash: force ? undefined : base });
      drafts.delete(draftKey);
      setData(file);
      setConflict(false);
      setSavedAt(Date.now());
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) setConflict(true);
      else setSaveError((e as Error).message);
    } finally {
      setSaving(false);
    }
  };
  // conflict → take the version from disk (own edits are dropped)
  const loadDisk = async () => {
    drafts.delete(draftKey);
    const file = await api.file(projectId, path).catch((e: Error) => (setSaveError(e.message), null));
    if (!file) return;
    setData(file);
    setDraft(file.content);
    setConflict(false);
    setEditorKey((k) => k + 1);
  };
  // live updates: someone (an agent, a shell) changed the file on disk
  const live = useRef({ data, dirty, editing, saving });
  live.current = { data, dirty, editing, saving };
  useFileChanges((m) => {
    if (!touches(m, projectId, path) || live.current.saving) return;
    setDiffRev((r) => r + 1);
    api.file(projectId, path).then(
      (file) => {
        setGone(false);
        const cur = live.current;
        if (file.hash && file.hash === cur.data?.hash) return; // our own save, or no real change
        // own unsaved edits are kept: the existing conflict bar offers to load or overwrite
        if (cur.dirty) return setConflict(true);
        setData(file);
        if (cur.editing && file.hash && !file.binary && !file.truncated) {
          setDraft(file.content);
          setExternal({ text: file.content, rev: Date.now() });
        } else if (cur.editing) stopEdit();
      },
      (e) => {
        if (e instanceof ApiError && e.status === 404) setGone(true);
      },
    );
  });

  useEffect(() => {
    if (!savedAt) return;
    const timer = setTimeout(() => setSavedAt(0), 1500);
    return () => clearTimeout(timer);
  }, [savedAt]);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex min-h-11 items-center gap-1 border-b border-line bg-surface px-2 py-1">
        {onBack ? (
          <button type="button" onClick={onBack} className="inline-flex size-9 shrink-0 items-center justify-center rounded-lg hover:bg-sunken lg:hidden" aria-label={t('common.back')}>
            <ChevronLeft className="size-5" />
          </button>
        ) : null}
        <div className="flex min-w-0 flex-1 items-center gap-0.5 overflow-x-auto px-1 font-mono text-[13px] whitespace-nowrap [scrollbar-width:none]">
          {rootName ? <span className="text-muted">{rootName}</span> : null}
          {parts.map((p, i) => {
            const last = i === parts.length - 1;
            return (
              <span key={i} className="flex items-center gap-0.5">
                {i > 0 || rootName ? <ChevronRight className="size-3.5 shrink-0 text-muted" /> : null}
                {last ? <FileIcon name={p} className="mr-1 size-4" /> : null}
                {last || !onCrumb ? (
                  <span className={last ? 'text-fg' : 'text-muted'}>{p}</span>
                ) : (
                  <button type="button" className="text-muted hover:text-fg hover:underline" onClick={() => onCrumb(parts.slice(0, i + 1).join('/'))}>
                    {p}
                  </button>
                )}
              </span>
            );
          })}
        </div>
        {editing ? (
          <div className="flex shrink-0 items-center gap-1.5">
            {savedAt && !dirty ? <span className="text-xs text-ok">{t('common.saved')}</span> : dirty ? <span className="hidden text-xs text-warn sm:inline">{t('files.unsaved')}</span> : null}
            {dirty ? (
              <>
                <Button size="sm" variant={confirmDiscard ? 'danger' : 'ghost'} onClick={() => (confirmDiscard ? stopEdit() : setConfirmDiscard(true))}>
                  {confirmDiscard ? t('files.discard') : t('common.cancel')}
                </Button>
                <Button size="sm" variant="primary" loading={saving} onClick={() => void save()}>
                  {t('common.save')}
                </Button>
              </>
            ) : null}
          </div>
        ) : null}
        {modes.length > 1 ? (
          <div className="flex shrink-0 rounded-lg bg-sunken p-0.5 text-sm">
            {modes.map((m) => (
              <button
                key={m}
                type="button"
                onClick={() => setMode(m)}
                className={`rounded-md px-2.5 py-1 ${mode === m ? 'bg-surface font-medium' : 'text-muted'}`}
              >
                {t(`files.${m}`)}
              </button>
            ))}
          </div>
        ) : null}
      </div>
      <div className="px-4 empty:hidden">
        <ErrorLine error={error} />
        <ErrorLine error={saveError} />
      </div>
      {gone ? (
        <div className="border-b border-warn/40 bg-warn/10 px-3 py-2 text-sm">{dirty ? t('files.goneDirty') : t('files.gone')}</div>
      ) : null}
      {editing && (conflict || stale) ? (
        <div className="flex flex-wrap items-center gap-2 border-b border-warn/40 bg-warn/10 px-3 py-2 text-sm">
          <span className="min-w-0 flex-1">{conflict ? t('files.conflict') : t('files.staleDraft')}</span>
          <Button size="sm" onClick={() => void loadDisk()}>
            {t('files.loadDisk')}
          </Button>
          <Button size="sm" variant="danger" loading={saving} onClick={() => void save(true)}>
            {t('files.overwrite')}
          </Button>
        </div>
      ) : editing && restored && dirty && !savedAt ? (
        <div className="border-b border-line bg-sunken px-3 py-1.5 text-xs text-muted">{t('files.draftRestored')}</div>
      ) : null}
      {editing && data && draft !== null && mode === 'file' ? (
        <div className="min-h-0 flex-1">
          <Suspense
            fallback={
              <div className="flex justify-center py-10">
                <Spinner />
              </div>
            }
          >
            <CodeEditor
              key={editorKey}
              path={path}
              initial={draft}
              line={line}
              vimMode={vimMode}
              tabSize={tabSize}
              relativeLines={relativeLines}
              // keyboard goes straight to the file; on a touch screen focus would pop up the keyboard
              autoFocus={!autoEdit || Boolean(line) || !coarsePointer()}
              external={external}
              onChange={change}
              onSave={() => void save()}
            />
          </Suspense>
        </div>
      ) : mode === 'diff' ? (
        <div className="min-h-0 flex-1 overflow-auto">{diff.loading ? <div className="p-4"><Spinner /></div> : <DiffView diff={diff.data?.diff ?? ''} path={path} />}</div>
      ) : loading && !data ? (
        <div className="flex justify-center py-10">
          <Spinner />
        </div>
      ) : data?.binary ? (
        <Empty title={t('files.binary')} hint={fmtSize(data.size)} />
      ) : data && mode === 'preview' ? (
        <div className="min-h-0 flex-1 overflow-auto px-5 py-4">
          <div className="mx-auto max-w-3xl">
            <Markdown text={dirty && draft !== null ? draft : data.content} />
          </div>
        </div>
      ) : data ? (
        <div ref={codeRef} className="min-h-0 flex-1 overflow-auto">
          <SelectionAsk container={codeRef} format={formatSel} />
          <table className="min-w-full font-mono text-[length:var(--code-size,12px)] leading-[1.6] [tab-size:var(--code-tab,2)]">
            <tbody>
              {lines.map((l, i) => (
                <tr key={i} data-line={i + 1} className={flash === i + 1 ? 'bg-accent/15' : undefined}>
                  <td className="sticky left-0 w-px bg-bg px-2 text-right text-muted/60 select-none">{i + 1}</td>
                  <td className="pr-4 pl-2 whitespace-pre">
                    <CodeLine tokens={tokens?.[i]} text={l} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {data.truncated ? <div className="px-4 py-2 text-sm text-muted">{t('files.truncated', { size: fmtSize(data.size) })}</div> : null}
        </div>
      ) : null}
    </div>
  );
}
