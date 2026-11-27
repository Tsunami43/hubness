import { useEffect, useRef, useState } from 'react';
import { Loader2, Mic, Square, X } from 'lucide-react';
import { api } from './api';
import { qk, useApi } from './query';
import { t } from '../i18n';

/**
 * Voice input for the chat input: record with the microphone, the hub transcribes
 * (Settings → Voice input), the text lands in the input to be checked before sending.
 *
 * Browsers give the microphone only to a secure page (HTTPS or localhost). Over plain HTTP
 * the button falls back to an audio file input: on Android it usually opens the recorder.
 */

/** A recording longer than this stops by itself (a forgotten microphone). */
const MAX_SECONDS = 10 * 60;
const MIME_TYPES = ['audio/webm;codecs=opus', 'audio/ogg;codecs=opus', 'audio/mp4', 'audio/webm'];

export function useVoiceSettings() {
  return useApi(qk.voice, api.voice, { staleTime: 60_000 });
}

const canRecord = () => window.isSecureContext && Boolean(navigator.mediaDevices?.getUserMedia) && typeof MediaRecorder !== 'undefined';

function micError(e: unknown): string {
  const name = (e as { name?: string })?.name;
  if (name === 'NotAllowedError' || name === 'SecurityError') return t('voice.denied');
  if (name === 'NotFoundError' || name === 'OverconstrainedError') return t('voice.noMic');
  return (e as Error)?.message || t('voice.failed');
}

type Phase = 'idle' | 'recording' | 'transcribing';

function useRecorder(onText: (text: string) => void, onError: (e: string | null) => void) {
  const [phase, setPhase] = useState<Phase>('idle');
  const [seconds, setSeconds] = useState(0);
  const rec = useRef<{ recorder: MediaRecorder; stream: MediaStream; timer: number; cancelled: boolean } | null>(null);
  const alive = useRef(true);
  const cb = useRef({ onText, onError });
  cb.current = { onText, onError };

  const release = () => {
    const r = rec.current;
    if (!r) return;
    clearInterval(r.timer);
    r.stream.getTracks().forEach((tr) => tr.stop());
    rec.current = null;
  };

  useEffect(
    () => () => {
      alive.current = false;
      if (rec.current) {
        rec.current.cancelled = true;
        if (rec.current.recorder.state !== 'inactive') rec.current.recorder.stop();
      }
      release();
    },
    [],
  );

  const send = async (audio: Blob) => {
    if (!audio.size) {
      setPhase('idle');
      return;
    }
    setPhase('transcribing');
    try {
      const text = await api.transcribe(audio);
      if (!alive.current) return;
      if (text) cb.current.onText(text);
      else cb.current.onError(t('voice.nothing'));
    } catch (e) {
      if (alive.current) cb.current.onError((e as Error).message);
    } finally {
      if (alive.current) setPhase('idle');
    }
  };

  const start = async () => {
    if (rec.current || phase !== 'idle') return;
    cb.current.onError(null);
    let stream: MediaStream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: { echoCancellation: true, noiseSuppression: true } });
    } catch (e) {
      cb.current.onError(micError(e));
      return;
    }
    if (!alive.current) {
      stream.getTracks().forEach((tr) => tr.stop());
      return;
    }
    const mimeType = MIME_TYPES.find((m) => MediaRecorder.isTypeSupported(m));
    const recorder = new MediaRecorder(stream, mimeType ? { mimeType } : undefined);
    const chunks: Blob[] = [];
    recorder.ondataavailable = (e) => e.data.size && chunks.push(e.data);
    recorder.onstop = () => {
      const cancelled = rec.current?.cancelled ?? true;
      release();
      if (!alive.current) return;
      if (cancelled) setPhase('idle');
      else void send(new Blob(chunks, { type: recorder.mimeType || mimeType || 'audio/webm' }));
    };
    const started = Date.now();
    const timer = window.setInterval(() => {
      const s = Math.floor((Date.now() - started) / 1000);
      setSeconds(s);
      if (s >= MAX_SECONDS && recorder.state === 'recording') recorder.stop();
    }, 250);
    rec.current = { recorder, stream, timer, cancelled: false };
    setSeconds(0);
    setPhase('recording');
    recorder.start(1000);
  };

  const stop = () => {
    const r = rec.current;
    if (r && r.recorder.state !== 'inactive') r.recorder.stop();
  };
  const cancel = () => {
    const r = rec.current;
    if (!r) return;
    r.cancelled = true;
    if (r.recorder.state !== 'inactive') r.recorder.stop();
    else release();
  };

  return { phase, seconds, start, stop, cancel, sendFile: send };
}

const clock = (s: number) => `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;

/**
 * Microphone in the chat input: tap to record, tap again to transcribe (✕ discards). Shown
 * only when voice input is set up. `onText` gets the transcript, errors go to `onError`.
 */
export function MicButton({ onText, onError }: { onText: (text: string) => void; onError: (e: string | null) => void }) {
  const voice = useVoiceSettings();
  const r = useRecorder(onText, onError);
  const file = useRef<HTMLInputElement>(null);
  if (!voice.data?.ready) return null;

  const btn = 'tap inline-flex size-8 shrink-0 items-center justify-center';
  if (r.phase === 'transcribing') {
    return (
      <span className={`${btn} text-muted`} title={t('voice.transcribing')} aria-label={t('voice.transcribing')}>
        <Loader2 className="size-4 animate-spin" />
      </span>
    );
  }
  if (r.phase === 'recording') {
    return (
      <span className="inline-flex shrink-0 items-center">
        <button type="button" onClick={r.cancel} title={t('voice.cancel')} aria-label={t('voice.cancel')} className={`${btn} text-muted hover:bg-sunken hover:text-fg`}>
          <X className="size-4" />
        </button>
        <button
          type="button"
          onClick={r.stop}
          title={t('voice.stop')}
          aria-label={t('voice.stop')}
          className="tap inline-flex h-8 shrink-0 items-center gap-1.5 border border-danger/60 px-2 text-xs text-danger tabular-nums hover:bg-danger/10"
        >
          <span className="size-2 animate-pulse rounded-full bg-danger" />
          {clock(r.seconds)}
          <Square className="size-3 fill-current" />
        </button>
      </span>
    );
  }
  const secure = canRecord();
  return (
    <>
      <button
        type="button"
        onClick={() => {
          if (secure) return void r.start();
          // no microphone over plain HTTP: an audio file (a phone offers its recorder)
          onError(t('voice.insecure'));
          file.current?.click();
        }}
        title={secure ? t('voice.record') : t('voice.insecure')}
        aria-label={t('voice.record')}
        className={`${btn} text-muted hover:bg-sunken hover:text-fg`}
      >
        <Mic className="size-4" />
      </button>
      <input
        ref={file}
        type="file"
        accept="audio/*"
        capture
        hidden
        onChange={(e) => {
          const f = e.target.files?.[0];
          e.target.value = '';
          if (!f) return;
          onError(null);
          void r.sendFile(f);
        }}
      />
    </>
  );
}
