import { useCallback, useEffect, useRef, useState, type ClipboardEvent, type DragEvent } from 'react';
import { FileText, Film, Paperclip, X } from 'lucide-react';
import type { AgentConfig, Attachment, Session } from '@hubness/shared';
import { api } from './api';
import { fmtSize } from './format';
import { t } from '../i18n';

/**
 * Files attached to a message before it is sent: pasted (Ctrl+V), dropped onto the input or
 * picked with the paperclip (on a phone — camera, gallery, files). They are uploaded right
 * before sending, so a new session gets its own folder first.
 */

export const MAX_FILE = 20 * 1024 * 1024;
const MAX_FILES = 20;

export interface PendingFile {
  key: string;
  file: File;
  /** Object URL for an image preview. */
  preview: string | null;
}

let seq = 0;

export function usePendingFiles() {
  const [files, setFiles] = useState<PendingFile[]>([]);
  const [error, setError] = useState<string | null>(null);
  const filesRef = useRef(files);
  filesRef.current = files;
  // free previews when the input goes away
  useEffect(() => () => filesRef.current.forEach((f) => f.preview && URL.revokeObjectURL(f.preview)), []);

  const add = useCallback((list: FileList | File[]) => {
    const picked = Array.from(list);
    if (!picked.length) return;
    const tooBig = picked.find((f) => f.size > MAX_FILE);
    setError(tooBig ? t('attach.tooBig', { name: tooBig.name, mb: MAX_FILE / 1024 / 1024 }) : null);
    setFiles((cur) =>
      [
        ...cur,
        ...picked
          .filter((f) => f.size <= MAX_FILE && f.size > 0)
          .map((file) => ({ key: `f${seq++}`, file, preview: file.type.startsWith('image/') ? URL.createObjectURL(file) : null })),
      ].slice(0, MAX_FILES),
    );
  }, []);
  const remove = (key: string) =>
    setFiles((cur) => {
      const f = cur.find((x) => x.key === key);
      if (f?.preview) URL.revokeObjectURL(f.preview);
      return cur.filter((x) => x.key !== key);
    });
  const clear = () =>
    setFiles((cur) => {
      cur.forEach((f) => f.preview && URL.revokeObjectURL(f.preview));
      return [];
    });

  /** Paste: files from the clipboard (a screenshot, copied files); plain text pastes as usual. */
  const onPaste = (e: ClipboardEvent) => {
    const list = Array.from(e.clipboardData.files);
    if (!list.length) return;
    e.preventDefault();
    // a pasted screenshot is called "image.png" everywhere: give it a readable name
    add(list.map((f) => (f.name === 'image.png' || !f.name ? new File([f], `screenshot-${new Date().toISOString().slice(0, 19).replace(/[:T]/g, '-')}.png`, { type: f.type }) : f)));
  };
  const [dragging, setDragging] = useState(false);
  const dropProps = {
    onDragOver: (e: DragEvent) => {
      if (!Array.from(e.dataTransfer.types).includes('Files')) return;
      e.preventDefault();
      setDragging(true);
    },
    onDragLeave: () => setDragging(false),
    onDrop: (e: DragEvent) => {
      if (!e.dataTransfer.files.length) return;
      e.preventDefault();
      setDragging(false);
      add(e.dataTransfer.files);
    },
  };
  return { files, add, remove, clear, onPaste, dropProps, dragging, error, setError };
}

/** Upload pending files into a session's folder. */
export async function uploadFiles(sessionId: string, files: PendingFile[]): Promise<Attachment[]> {
  const out: Attachment[] = [];
  for (const { file } of files) out.push(await api.uploadAttachment(sessionId, file));
  return out;
}

/** Send a message with files to an existing session. */
export async function sendWithFiles(sessionId: string, text: string, files: PendingFile[]): Promise<Session> {
  return api.send(sessionId, text, files.length ? await uploadFiles(sessionId, files) : undefined);
}

/**
 * Start a session with a first message. Without files — one request; with files the session
 * is created first (its folder is where the files go), then they are uploaded and sent.
 */
export async function createWithFiles(input: { projectId?: string | null; groupId?: string | null; agent: AgentConfig }, text: string, files: PendingFile[]): Promise<Session> {
  if (!files.length) return api.createSession({ ...input, prompt: text });
  const title = (text.trim() || files.map((f) => f.file.name).join(', ')).replace(/\s+/g, ' ').slice(0, 60);
  const s = await api.createSession({ ...input, title });
  return sendWithFiles(s.id, text, files);
}

/** Paperclip: a hidden file input behind a button (camera/gallery/files on a phone). */
export function AttachButton({ onPick, className = '' }: { onPick: (files: FileList) => void; className?: string }) {
  const input = useRef<HTMLInputElement>(null);
  return (
    <>
      <button
        type="button"
        onClick={() => input.current?.click()}
        title={t('attach.add')}
        aria-label={t('attach.add')}
        className={`tap inline-flex size-8 shrink-0 items-center justify-center text-muted hover:bg-sunken hover:text-fg ${className}`}
      >
        <Paperclip className="size-4" />
      </button>
      <input
        ref={input}
        type="file"
        multiple
        className="hidden"
        onChange={(e) => {
          if (e.target.files) onPick(e.target.files);
          e.target.value = '';
        }}
      />
    </>
  );
}

const fileIcon = (mime: string) => (mime.startsWith('video/') ? Film : FileText);

/** Pending files above the input: image thumbnails and file chips, each with ×. */
export function PendingFiles({ files, onRemove }: { files: PendingFile[]; onRemove: (key: string) => void }) {
  if (!files.length) return null;
  return (
    <div className="flex flex-wrap gap-2 pb-2">
      {files.map((f) => {
        const Icon = fileIcon(f.file.type);
        return (
          <div key={f.key} className="relative border border-line bg-sunken" title={`${f.file.name} · ${fmtSize(f.file.size)}`}>
            {f.preview ? (
              <img src={f.preview} alt={f.file.name} className="size-16 object-cover" />
            ) : (
              <div className="flex h-16 w-40 items-center gap-2 px-2">
                <Icon className="size-5 shrink-0 text-muted" />
                <div className="min-w-0 text-xs">
                  <div className="truncate text-fg">{f.file.name}</div>
                  <div className="text-muted">{fmtSize(f.file.size)}</div>
                </div>
              </div>
            )}
            <button
              type="button"
              onClick={() => onRemove(f.key)}
              aria-label={t('attach.remove')}
              title={t('attach.remove')}
              className="absolute -top-2 -right-2 inline-flex size-5 items-center justify-center rounded-full border border-line bg-surface text-muted hover:text-fg"
            >
              <X className="size-3" />
            </button>
          </div>
        );
      })}
    </div>
  );
}

/** Attachments of a sent message in the feed: images open full size, files download. */
export function MessageAttachments({ sessionId, list }: { sessionId: string; list: Attachment[] }) {
  const url = (a: Attachment) => `/api/sessions/${sessionId}/attachments/${encodeURIComponent(a.id)}`;
  return (
    <div className="mt-2 flex flex-wrap gap-2">
      {list.map((a) => {
        const Icon = fileIcon(a.mime);
        return a.mime.startsWith('image/') && !a.mime.includes('svg') ? (
          <a key={a.id} href={url(a)} target="_blank" rel="noopener" title={a.name} className="block border border-line">
            <img src={url(a)} alt={a.name} loading="lazy" className="max-h-40 max-w-[14rem] object-cover" />
          </a>
        ) : (
          <a key={a.id} href={url(a)} target="_blank" rel="noopener" className="flex max-w-xs items-center gap-2 border border-line bg-sunken px-2 py-1.5 text-xs hover:text-fg" title={a.path}>
            <Icon className="size-4 shrink-0 text-muted" />
            <span className="min-w-0 truncate text-fg">{a.name}</span>
            <span className="shrink-0 text-muted">{fmtSize(a.size)}</span>
          </a>
        );
      })}
    </div>
  );
}
