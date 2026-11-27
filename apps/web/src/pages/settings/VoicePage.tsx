import { useEffect, useRef, useState } from 'react';
import { Check, CircleAlert, CircleCheck, Download, Hammer, Square, Trash2 } from 'lucide-react';
import type { JobInfo, JobStreamMsg, VoiceProvider, VoiceSettings, VoiceSettingsInput } from '@hubness/shared';
import { api } from '../../lib/api';
import { fmtSize } from '../../lib/format';
import { useVoiceSettings } from '../../lib/voice';
import { liveStream } from '../../lib/stream';
import { Button, ErrorLine, Field, IconButton, inputCls, Spinner } from '../../components/ui';
import { Select } from '../../components/Select';
import { sheetTermSize } from '../../components/Terminal';
import { confirmDialog } from '../../components/Confirm';
import { getLang, t } from '../../i18n';
import type { MessageKey } from '../../i18n/en';

const PROVIDERS: VoiceProvider[] = ['off', 'local', 'api'];
const LANGUAGES = ['auto', 'en', 'ru', 'uk', 'de', 'fr', 'es', 'it', 'pt', 'pl', 'tr', 'zh', 'ja', 'ko'];
const RECOMMENDED_MODEL = 'large-v3-turbo-q5_0';

/** OpenAI-compatible services: one click fills the URL and the model. */
const PRESETS = [
  { name: 'Groq', url: 'https://api.groq.com/openai/v1', model: 'whisper-large-v3-turbo' },
  { name: 'Mistral', url: 'https://api.mistral.ai/v1', model: 'voxtral-mini-latest' },
];

/**
 * Settings → Voice input: where speech from the microphone in the chat input is transcribed —
 * whisper.cpp on this machine (the hub builds it and downloads models) or an OpenAI-compatible
 * service. Stored on the server, the same on every device.
 */
export function VoicePage() {
  const { data: s, setData, error: loadError } = useVoiceSettings();
  const [error, setError] = useState<string | null>(null);
  const [steps, setSteps] = useState<Step[] | null>(null);
  if (!s) return loadError ? <ErrorLine error={loadError} /> : <Spinner />;

  const save = async (patch: VoiceSettingsInput) => {
    setError(null);
    try {
      setData(await api.saveVoice(patch));
      return true;
    } catch (e) {
      setError((e as Error).message);
      return false;
    }
  };
  const reload = () => api.voice().then(setData, (e) => setError((e as Error).message));
  // one click on a model does the rest in one overlay: download it, choose it, build whisper.cpp
  const needBuild = !s.local.binary && !s.local.missingBuildTools.length;
  const pick = async (m: VoiceSettings['local']['models'][number]) => {
    const next: Step[] = [];
    if (!m.installed) next.push({ kind: 'download', id: m.id, label: m.label });
    else if (m.id !== s.local.model && !(await save({ localModel: m.id }))) return;
    if (needBuild) next.push({ kind: 'build' });
    if (next.length) setSteps(next);
  };

  return (
    <div className="space-y-6">
      <p className="text-sm text-muted">{t('voice.intro')}</p>
      <section>
        <h2 className="mb-2 font-semibold">{t('voice.provider')}</h2>
        <ul className="divide-y divide-line border border-line bg-surface">
          {PROVIDERS.map((p) => (
            <li key={p}>
              <button type="button" onClick={() => p !== s.provider && void save({ provider: p })} className="flex w-full items-start gap-3 px-4 py-3 text-left hover:bg-sunken">
                <span className="min-w-0 flex-1">
                  <span className="block font-medium">{t(`voice.provider.${p}` as MessageKey)}</span>
                  <span className="block text-xs text-muted">{t(`voice.provider.${p}Hint` as MessageKey)}</span>
                </span>
                {p === s.provider ? <Check className="mt-0.5 size-5 shrink-0 text-accent" /> : null}
              </button>
            </li>
          ))}
        </ul>
      </section>

      {s.provider !== 'off' ? (
        <>
          <Status s={s} />
          <Field label={t('voice.language')} hint={t('voice.languageHint')}>
            <Select
              value={s.language}
              options={LANGUAGES.map((l) => ({ value: l, label: l === 'auto' ? t('voice.languageAuto') : `${langName(l)} (${l})` }))}
              onChange={(language) => void save({ language })}
              label={t('voice.language')}
            />
          </Field>
        </>
      ) : null}

      {s.provider === 'local' ? (
        <LocalSection
          s={s}
          onPick={(m) => void pick(m)}
          onInstall={() => setSteps([{ kind: 'build' }])}
          onDelete={async (id, label) => {
            if (!(await confirmDialog({ message: t('voice.deleteModelConfirm', { name: label }), confirmLabel: t('common.delete'), danger: true }))) return;
            api.voiceDeleteModel(id).then(setData, (e) => setError((e as Error).message));
          }}
        />
      ) : null}
      {s.provider === 'api' ? <ApiSection s={s} save={save} /> : null}

      <ErrorLine error={error} />
      {s.provider !== 'off' ? <p className="text-xs text-muted">{t('voice.httpsHint')}</p> : null}
      {steps ? (
        <SetupOverlay
          steps={steps}
          onFinish={() => {
            setSteps(null);
            void reload();
          }}
        />
      ) : null}
    </div>
  );
}

function langName(code: string): string {
  try {
    return new Intl.DisplayNames([getLang()], { type: 'language' }).of(code) ?? code;
  } catch {
    return code;
  }
}

/** Ready, or the first thing still missing. */
function Status({ s }: { s: VoiceSettings }) {
  let problem: string | null = null;
  if (s.provider === 'local') {
    if (!s.local.binary) problem = t('voice.needWhisper');
    else if (!s.local.ffmpeg) problem = t('voice.needFfmpeg');
    else if (!s.local.models.find((m) => m.id === s.local.model)?.installed) problem = t('voice.needModel');
  } else if (s.provider === 'api' && !s.ready) problem = t('voice.needApi');
  return problem ? (
    <p className="flex items-start gap-2 border border-warn/40 bg-warn/10 px-3 py-2 text-sm">
      <CircleAlert className="mt-0.5 size-4 shrink-0 text-warn" />
      {problem}
    </p>
  ) : (
    <p className="flex items-start gap-2 border border-ok/40 bg-ok/10 px-3 py-2 text-sm">
      <CircleCheck className="mt-0.5 size-4 shrink-0 text-ok" />
      {t('voice.ready')}
    </p>
  );
}

function LocalSection({
  s,
  onPick,
  onInstall,
  onDelete,
}: {
  s: VoiceSettings;
  onPick: (m: VoiceSettings['local']['models'][number]) => void;
  onInstall: () => void;
  onDelete: (id: string, label: string) => void;
}) {
  const { local } = s;
  const canBuild = !local.missingBuildTools.length;
  return (
    <>
      <section>
        <h2 className="mb-1 font-semibold">{t('voice.model')}</h2>
        <p className="mb-2 text-xs text-muted">{t('voice.modelHint')}</p>
        <ul className="divide-y divide-line border border-line bg-surface">
          {local.models.map((m) => (
            <li key={m.id} className="flex items-center gap-2 pr-2">
              {/* a click downloads the model if needed, chooses it and builds whisper.cpp if needed */}
              <button type="button" onClick={() => onPick(m)} className="flex min-w-0 flex-1 items-center gap-3 px-4 py-2.5 text-left hover:bg-sunken">
                <span className="min-w-0 flex-1">
                  <span className={`block text-sm font-medium ${m.installed ? '' : 'text-muted'}`}>
                    {m.label}
                    {m.id === RECOMMENDED_MODEL ? <span className="ml-2 text-xs font-normal text-muted">{t('agentSettings.recommended')}</span> : null}
                  </span>
                  <span className="block text-xs text-muted">
                    {fmtSize(m.size)} · {m.installed ? t('voice.installed') : t('voice.notInstalled')}
                  </span>
                </span>
                {m.installed && m.id === local.model ? <Check className="size-5 shrink-0 text-accent" /> : null}
              </button>
              {m.installed ? (
                <IconButton label={t('common.delete')} className="size-8" onClick={() => onDelete(m.id, m.label)}>
                  <Trash2 className="size-3.5" />
                </IconButton>
              ) : (
                <IconButton label={t('voice.download')} className="size-8" onClick={() => onPick(m)}>
                  <Download className="size-3.5" />
                </IconButton>
              )}
            </li>
          ))}
        </ul>
      </section>

      <section className="space-y-2">
        <h2 className="font-semibold">whisper.cpp</h2>
        {local.binary ? (
          <div className="flex flex-wrap items-center gap-3 border border-line bg-surface px-4 py-3">
            <div className="min-w-0 flex-1">
              <div className="text-sm">{local.managed ? t('voice.whisperManaged') : t('voice.whisperSystem')}</div>
              <div className="truncate font-mono text-xs text-muted">{local.binary}</div>
            </div>
            {local.managed ? (
              <Button size="sm" onClick={onInstall} disabled={!canBuild}>
                <Hammer className="size-3.5" />
                {t('voice.rebuild')}
              </Button>
            ) : null}
          </div>
        ) : (
          <p className="text-xs text-muted">{t('voice.buildAuto')}</p>
        )}
        {!canBuild && (!local.binary || local.managed) ? <p className="text-xs text-warn">{t('voice.buildMissing', { tools: local.missingBuildTools.join(', ') })}</p> : null}
        {!local.ffmpeg ? <p className="text-xs text-warn">{t('voice.ffmpegMissing')}</p> : null}
      </section>
    </>
  );
}

// eslint-disable-next-line no-control-regex
const ANSI = /\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07]*\x07/g;

/** The last lines of a job's output as plain text (progress lines rewritten with \r keep their last state). */
function tailLines(out: string, n: number) {
  return out
    .replace(ANSI, '')
    .split('\n')
    .map((l) => l.split('\r').filter(Boolean).pop() ?? '')
    .filter((l) => l.trim())
    .slice(-n);
}

const clock = (secs: number) => `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, '0')}`;

/** A step of setting up local voice input, each one a terminal job on the server. */
type Step = { kind: 'download'; id: string; label: string } | { kind: 'build' };

/** curl's progress bar ends with the percentage. */
const percentOf = (line: string | undefined) => {
  const m = line?.match(/(\d{1,3}(?:\.\d)?)%\s*$/);
  return m ? Math.min(100, Number(m[1])) : null;
};

/** A ring: filled to `percent`, or spinning while the progress is unknown. */
function Ring({ percent }: { percent: number | null }) {
  if (percent === null) return <span aria-hidden className="size-12 shrink-0 animate-spin rounded-full border-4 border-line border-t-accent" />;
  const c = 2 * Math.PI * 20;
  return (
    <span className="relative inline-flex size-12 shrink-0 items-center justify-center">
      <svg viewBox="0 0 48 48" className="absolute inset-0 -rotate-90" aria-hidden>
        <circle cx="24" cy="24" r="20" fill="none" strokeWidth="4" className="stroke-line" />
        <circle cx="24" cy="24" r="20" fill="none" strokeWidth="4" className="stroke-accent" strokeDasharray={c} strokeDashoffset={c * (1 - percent / 100)} />
      </svg>
      <span className="font-mono text-[10px] tabular-nums">{Math.round(percent)}%</span>
    </span>
  );
}

/**
 * Blocks the page while voice input gets ready: downloads the model (then chooses it) and
 * builds whisper.cpp, one step after another, with the progress, the elapsed time and the
 * current output line; the log when a step fails.
 */
function SetupOverlay({ steps, onFinish }: { steps: Step[]; onFinish: () => void }) {
  const [index, setIndex] = useState(0);
  const [job, setJob] = useState<JobInfo | null>(null);
  const out = useRef('');
  const [lines, setLines] = useState<string[]>([]);
  const [failed, setFailed] = useState<string | null>(null);
  const [secs, setSecs] = useState(0);
  const step = steps[index]!;

  // start the current step's job
  useEffect(() => {
    let live = true;
    out.current = '';
    setLines([]);
    setJob(null);
    const size = sheetTermSize(24);
    (step.kind === 'download' ? api.voiceDownload(step.id, size) : api.voiceInstall(size)).then(
      (j) => live && setJob(j),
      (e) => live && setFailed((e as Error).message),
    );
    return () => {
      live = false;
    };
  }, [index]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    const started = Date.now();
    const timer = setInterval(() => setSecs(Math.round((Date.now() - started) / 1000)), 1000);
    return () => clearInterval(timer);
  }, []);

  // follow it; on success choose the downloaded model and go on
  useEffect(() => {
    if (!job) return;
    let done = false;
    const finish = (code: number | null) => {
      if (done) return;
      done = true;
      stop();
      if (code !== 0) return setFailed('');
      void (async () => {
        if (step.kind === 'download') await api.saveVoice({ localModel: step.id }).catch(() => {});
        if (index + 1 < steps.length) setIndex(index + 1);
        else onFinish();
      })();
    };
    const stop = liveStream(`/api/jobs/${job.id}/stream`, (data) => {
      const m = JSON.parse(data) as JobStreamMsg;
      if (m.type === 'output') out.current += m.data;
      else if (m.type === 'snapshot') out.current = m.output;
      setLines(tailLines(out.current, 14));
      if (m.type === 'exit') finish(m.code);
      // the exit can fall between two connections: a (re)connect then finds the job already exited
      else if (m.type === 'snapshot' && m.job.status === 'exited') finish(m.job.exitCode);
    });
    return () => {
      done = true;
      stop();
    };
  }, [job?.id]); // eslint-disable-line react-hooks/exhaustive-deps

  const title = step.kind === 'download' ? t('voice.downloading', { name: step.label }) : t('voice.building');
  const percent = step.kind === 'download' ? percentOf(lines.at(-1)) : null;
  return (
    <div className="fixed inset-0 z-[70] flex items-center justify-center bg-black/60 p-4" role="alertdialog" aria-modal="true" aria-label={title}>
      <div className="w-full max-w-md space-y-4 border border-line bg-surface p-5">
        {failed !== null ? (
          <>
            <p className="flex items-center gap-2 font-semibold text-danger">
              <CircleAlert className="size-5" /> {step.kind === 'download' ? t('voice.downloadFailed') : t('voice.buildFailed')}
            </p>
            <pre className="max-h-64 overflow-auto bg-bg p-2 font-mono text-xs whitespace-pre-wrap text-muted">{failed || lines.join('\n')}</pre>
            <Button variant="primary" onClick={onFinish}>
              {t('common.close')}
            </Button>
          </>
        ) : (
          <>
            <div className="flex items-center gap-4">
              <Ring percent={percent} />
              <div className="min-w-0">
                {steps.length > 1 ? <p className="text-xs text-muted">{t('voice.step', { n: index + 1, total: steps.length })}</p> : null}
                <p className="font-semibold">{title}</p>
                <p className="text-xs text-muted">{step.kind === 'download' ? t('voice.downloadingHint') : t('voice.buildingHint')}</p>
              </div>
              <span className="ml-auto font-mono text-sm text-muted tabular-nums">{clock(secs)}</span>
            </div>
            {step.kind === 'build' ? <p className="truncate font-mono text-xs text-muted">{lines.at(-1) ?? ''}</p> : null}
            <Button size="sm" disabled={!job} onClick={() => job && void api.jobKill(job.id)}>
              <Square className="size-3.5" /> {t('term.stop')}
            </Button>
          </>
        )}
      </div>
    </div>
  );
}

function ApiSection({ s, save }: { s: VoiceSettings; save: (patch: VoiceSettingsInput) => Promise<boolean> }) {
  const [url, setUrl] = useState(s.api.url);
  const [model, setModel] = useState(s.api.model);
  const [key, setKey] = useState('');
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    setUrl(s.api.url);
    setModel(s.api.model);
  }, [s.api.url, s.api.model]);
  const dirty = url.trim() !== s.api.url || model.trim() !== s.api.model || key.trim() !== '';

  const submit = async () => {
    setBusy(true);
    const ok = await save({ apiUrl: url, apiModel: model, ...(key.trim() ? { apiKey: key } : {}) });
    setBusy(false);
    if (ok) setKey('');
  };

  return (
    <section className="space-y-3">
      <h2 className="font-semibold">{t('voice.service')}</h2>
      <div className="flex flex-wrap gap-2">
        {PRESETS.map((p) => (
          <Button
            key={p.name}
            size="sm"
            variant={url === p.url ? 'primary' : 'secondary'}
            onClick={() => {
              setUrl(p.url);
              setModel(p.model);
            }}
          >
            {p.name}
          </Button>
        ))}
      </div>
      <Field label={t('voice.apiUrl')} hint={t('voice.apiUrlHint')}>
        <input className={inputCls} value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://api.groq.com/openai/v1" spellCheck={false} />
      </Field>
      <Field label={t('voice.apiModel')}>
        <input className={inputCls} value={model} onChange={(e) => setModel(e.target.value)} placeholder="whisper-large-v3-turbo" spellCheck={false} />
      </Field>
      <Field label={t('voice.apiKey')} hint={t('voice.apiKeyHint')}>
        <input
          className={inputCls}
          type="password"
          autoComplete="off"
          value={key}
          onChange={(e) => setKey(e.target.value)}
          placeholder={s.api.hasKey ? t('voice.apiKeySaved') : ''}
        />
      </Field>
      <div className="flex flex-wrap gap-2">
        <Button variant="primary" onClick={() => void submit()} disabled={!dirty} loading={busy}>
          {t('common.save')}
        </Button>
        {s.api.hasKey ? (
          <Button variant="ghost" onClick={() => void save({ apiKey: null })}>
            {t('voice.apiKeyRemove')}
          </Button>
        ) : null}
      </div>
    </section>
  );
}
