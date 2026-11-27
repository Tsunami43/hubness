import { spawn } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { availableParallelism, tmpdir } from 'node:os';
import { delimiter, join } from 'node:path';
import type { JobInfo, VoiceModel, VoiceProvider, VoiceSettings, VoiceSettingsInput } from '@hubness/shared';
import { childEnv, config } from './config.ts';
import { kv } from './db/index.ts';
import { HttpError } from './errors.ts';
import { decrypt, encrypt } from './secrets.ts';
import { shq, startJob } from './terminal.ts';

/**
 * Voice input: a recording from the browser becomes text for the chat input. Two providers:
 * - `local` — whisper.cpp on this machine. The hub finds `whisper-cli` on PATH or builds its
 *   own copy into `<data>/whisper/bin` (a terminal job), downloads ggml models into
 *   `<data>/whisper/models` and converts the recording to 16 kHz WAV with ffmpeg;
 * - `api` — any OpenAI-compatible `/audio/transcriptions` endpoint (OpenAI, Groq, speaches,
 *   whisper-server…); the key is encrypted like project secrets.
 */

export const MAX_AUDIO = 25 * 1024 * 1024;
const KEY = 'settings.voice';
// AAD namespace of the encrypted API key (the "project id" of secrets.encrypt)
const NS = 'voice';
const MODELS_BASE = 'https://huggingface.co/ggerganov/whisper.cpp/resolve/main';
const WHISPER_REPO = 'https://github.com/ggml-org/whisper.cpp';
const WHISPER_ARCHIVE = 'https://github.com/ggml-org/whisper.cpp/archive/refs/heads/master.tar.gz';
/** The portable cmake the hub downloads when the host has none (Kitware's release archives). */
const CMAKE_VERSION = '3.31.6';
const PROVIDERS: VoiceProvider[] = ['off', 'local', 'api'];

/** Models offered in Settings, smallest first; ids are ggml file names without `ggml-` and `.bin`. */
export const MODELS: { id: string; label: string; size: number }[] = [
  { id: 'tiny', label: 'Tiny', size: 77_691_713 },
  { id: 'base', label: 'Base', size: 147_951_465 },
  { id: 'small', label: 'Small', size: 487_601_967 },
  { id: 'large-v3-turbo-q5_0', label: 'Large v3 Turbo (q5)', size: 574_041_195 },
  { id: 'large-v3-turbo', label: 'Large v3 Turbo', size: 1_624_555_275 },
];
const DEFAULT_MODEL = 'large-v3-turbo-q5_0';

interface Stored {
  provider?: VoiceProvider;
  language?: string;
  localModel?: string;
  apiUrl?: string;
  apiModel?: string;
  /** Encrypted with secrets.encrypt. */
  apiKey?: string;
}

const dirs = () => {
  const root = join(config.dataDir, 'whisper');
  return { root, bin: join(root, 'bin'), models: join(root, 'models'), src: join(root, 'src') };
};
const modelFile = (id: string) => join(dirs().models, `ggml-${id}.bin`);
const managedBinary = () => join(dirs().bin, 'whisper-cli');

function which(bin: string): string | null {
  for (const dir of (process.env.PATH ?? '').split(delimiter)) {
    if (!dir) continue;
    const p = join(dir, bin);
    try {
      if (statSync(p).isFile()) return p;
    } catch {}
  }
  return null;
}

const isFile = (p: string) => {
  try {
    return statSync(p).isFile();
  } catch {
    return false;
  }
};

/** whisper-cli to run: the hub's own build first, then PATH (`whisper-cpp` is the Homebrew/Nix name). */
function findBinary(): { path: string; managed: boolean } | null {
  if (isFile(managedBinary())) return { path: managedBinary(), managed: true };
  for (const name of ['whisper-cli', 'whisper-cpp']) {
    const p = which(name);
    if (p) return { path: p, managed: false };
  }
  return null;
}

/**
 * What the host must have to build whisper.cpp; cmake isn't listed (the hub downloads a
 * portable one) and neither is git (the source comes as an archive).
 */
function missingBuildTools(): string[] {
  const out: string[] = [];
  if (!['c++', 'g++', 'clang++'].some((c) => which(c))) out.push('c++');
  if (!which('make')) out.push('make');
  if (!which('curl') && !which('wget')) out.push('curl');
  if (!which('tar')) out.push('tar');
  return out;
}

const stored = (): Stored => kv.get<Stored>(KEY)?.value ?? {};

export function voiceSettings(): VoiceSettings {
  const s = stored();
  const provider = s.provider && PROVIDERS.includes(s.provider) ? s.provider : 'off';
  const model = MODELS.some((m) => m.id === s.localModel) ? s.localModel! : DEFAULT_MODEL;
  const bin = findBinary();
  const ffmpeg = Boolean(which('ffmpeg'));
  const models: VoiceModel[] = MODELS.map((m) => ({ ...m, installed: isFile(modelFile(m.id)) }));
  const apiUrl = s.apiUrl ?? '';
  const apiModel = s.apiModel ?? '';
  const ready =
    provider === 'local' ? Boolean(bin && ffmpeg && models.find((m) => m.id === model)?.installed) : provider === 'api' ? Boolean(apiUrl && apiModel) : false;
  return {
    provider,
    language: s.language ?? 'auto',
    ready,
    local: { model, binary: bin?.path ?? null, managed: bin?.managed ?? false, ffmpeg, missingBuildTools: missingBuildTools(), models },
    api: { url: apiUrl, model: apiModel, hasKey: Boolean(s.apiKey) },
  };
}

const LANG_RE = /^(auto|[a-z]{2,3})$/;
const CONTROL_RE = /[\0-\x1f\x7f]/;

export function setVoiceSettings(b: VoiceSettingsInput): VoiceSettings {
  const next = stored();
  if (b.provider !== undefined) {
    if (!PROVIDERS.includes(b.provider)) throw new HttpError(400, 'provider: off | local | api');
    next.provider = b.provider;
  }
  if (b.language !== undefined) {
    if (typeof b.language !== 'string' || !LANG_RE.test(b.language)) throw new HttpError(400, 'language: auto or a language code like en');
    next.language = b.language;
  }
  if (b.localModel !== undefined) {
    if (!MODELS.some((m) => m.id === b.localModel)) throw new HttpError(400, `Unknown model: ${String(b.localModel)}`);
    next.localModel = b.localModel;
  }
  if (b.apiUrl !== undefined) {
    const url = typeof b.apiUrl === 'string' ? b.apiUrl.trim() : null;
    if (url === null || url.length > 2000 || (url && !/^https?:\/\/[^\s/]+/.test(url))) throw new HttpError(400, 'apiUrl: an http:// or https:// URL');
    next.apiUrl = url;
  }
  if (b.apiModel !== undefined) {
    const model = typeof b.apiModel === 'string' ? b.apiModel.trim() : null;
    if (model === null || model.length > 200 || CONTROL_RE.test(model)) throw new HttpError(400, 'apiModel: at most 200 characters');
    next.apiModel = model;
  }
  if (b.apiKey !== undefined) {
    const key = typeof b.apiKey === 'string' ? b.apiKey.trim() : b.apiKey;
    if (key === null || key === '') delete next.apiKey;
    else if (typeof key !== 'string' || key.length > 4096 || CONTROL_RE.test(key)) throw new HttpError(400, 'apiKey: at most 4096 characters');
    else next.apiKey = encrypt(NS, 'apiKey', key);
  }
  kv.set(KEY, next);
  return voiceSettings();
}

// ---------- whisper.cpp: build and models (terminal jobs) ----------

/** Build whisper-cli from source into the data folder (static, tuned for this CPU). */
export function installWhisper(size: { cols?: number; rows?: number }): JobInfo {
  const missing = missingBuildTools();
  if (missing.length) throw new HttpError(400, `Missing to build whisper.cpp: ${missing.join(', ')}`);
  const script = buildScript(dirs().root, managedBinary(), Math.max(1, Math.min(16, availableParallelism())));
  return startJob({ title: 'whisper.cpp · build', command: script, display: `build whisper.cpp from ${WHISPER_REPO}`, cols: size.cols, rows: size.rows });
}

/** The build as a shell script (the same text as `build_script` in `voice.rs`). */
export function buildScript(root: string, bin: string, jobs: number): string {
  const src = `${root}/src`;
  const build = `${src}/build`;
  const cmakeDir = `${root}/cmake`;
  const pick = `CMAKE=${shq(`${cmakeDir}/bin/cmake`)}; [ -x "$CMAKE" ] || CMAKE=${shq(`${cmakeDir}/CMake.app/Contents/bin/cmake`)}`;
  return [
    'set -e',
    'fetch() { if command -v curl >/dev/null 2>&1; then curl -fsSL "$1"; else wget -qO- "$1"; fi; }',
    // cmake: the host's, else a portable one kept next to the build
    'if command -v cmake >/dev/null 2>&1; then CMAKE=cmake; else',
    `  ${pick}`,
    '  if [ ! -x "$CMAKE" ]; then',
    '    case "$(uname -s)-$(uname -m)" in',
    '      Linux-x86_64) A=linux-x86_64 ;; Linux-aarch64|Linux-arm64) A=linux-aarch64 ;; Darwin-*) A=macos-universal ;;',
    "      *) echo 'No portable cmake for this platform: install cmake'; exit 1 ;;",
    '    esac',
    `    echo "Downloading cmake ${CMAKE_VERSION}…"`,
    `    rm -rf ${shq(cmakeDir)} && mkdir -p ${shq(cmakeDir)}`,
    `    fetch "https://github.com/Kitware/CMake/releases/download/v${CMAKE_VERSION}/cmake-${CMAKE_VERSION}-$A.tar.gz" | tar -xz --strip-components=1 -C ${shq(cmakeDir)}`,
    `    ${pick}`,
    '  fi',
    'fi',
    "echo 'Downloading whisper.cpp…'",
    `rm -rf ${shq(src)} && mkdir -p ${shq(src)}`,
    `fetch ${WHISPER_ARCHIVE} | tar -xz --strip-components=1 -C ${shq(src)}`,
    `"$CMAKE" -S ${shq(src)} -B ${shq(build)} -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF -DWHISPER_BUILD_TESTS=OFF`,
    `"$CMAKE" --build ${shq(build)} --config Release --target whisper-cli -j ${jobs}`,
    `mkdir -p ${shq(`${root}/bin`)}`,
    `cp ${shq(`${build}/bin/whisper-cli`)} ${shq(`${bin}.new`)}`,
    `mv ${shq(`${bin}.new`)} ${shq(bin)}`,
    `rm -rf ${shq(src)}`,
    `echo; echo ${shq(`whisper.cpp is installed: ${bin}`)}`,
  ].join('\n');
}

const modelOr404 = (id: string) => {
  const m = MODELS.find((x) => x.id === id);
  if (!m) throw new HttpError(404, `Unknown model: ${id}`);
  return m;
};

/** Download a ggml model (curl, or wget) into the data folder; a partial file never counts as installed. */
export function downloadModel(id: string, size: { cols?: number; rows?: number }): JobInfo {
  modelOr404(id);
  const file = modelFile(id);
  const part = `${file}.part`;
  const url = `${MODELS_BASE}/ggml-${id}.bin`;
  const script = [
    'set -e',
    `mkdir -p ${shq(dirs().models)}`,
    `if command -v curl >/dev/null 2>&1; then curl -fL --progress-bar -o ${shq(part)} ${shq(url)}; else wget -O ${shq(part)} ${shq(url)}; fi`,
    `mv ${shq(part)} ${shq(file)}`,
  ].join('\n');
  return startJob({ title: `whisper.cpp · ${id}`, command: script, display: `download ${url}`, cols: size.cols, rows: size.rows });
}

export function deleteModel(id: string): VoiceSettings {
  modelOr404(id);
  rmSync(modelFile(id), { force: true });
  rmSync(`${modelFile(id)}.part`, { force: true });
  return voiceSettings();
}

// ---------- transcription ----------

function run(cmd: string, args: string[], timeoutMs: number): Promise<{ code: number | null; err: string }> {
  return new Promise((resolve) => {
    const p = spawn(cmd, args, { stdio: ['ignore', 'ignore', 'pipe'], env: childEnv() });
    let err = '';
    const timer = setTimeout(() => p.kill('SIGKILL'), timeoutMs);
    p.stderr.on('data', (d: Buffer) => (err = (err + d.toString()).slice(-4000)));
    p.on('error', (e) => (clearTimeout(timer), resolve({ code: null, err: e.message })));
    p.on('close', (code) => (clearTimeout(timer), resolve({ code, err })));
  });
}

const lastLine = (s: string) =>
  s
    .trim()
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean)
    .pop() ?? '';

/** whisper.cpp prints one segment per line and marks silence: plain text for the input. */
export function cleanTranscript(raw: string): string {
  return raw
    .replace(/\[BLANK_AUDIO\]/g, ' ')
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean)
    .join(' ')
    .replace(/\s+/g, ' ')
    .trim();
}

async function transcribeLocal(audio: Uint8Array, s: VoiceSettings): Promise<string> {
  if (!s.local.binary) throw new HttpError(409, 'whisper.cpp is not installed: Settings → Voice input');
  if (!s.local.ffmpeg) throw new HttpError(409, 'ffmpeg is not installed');
  const model = modelFile(s.local.model);
  if (!existsSync(model)) throw new HttpError(409, 'The model is not downloaded: Settings → Voice input');
  const dir = mkdtempSync(join(tmpdir(), 'hubness-voice-'));
  try {
    const input = join(dir, 'input');
    const wav = join(dir, 'audio.wav');
    writeFileSync(input, audio);
    const conv = await run('ffmpeg', ['-nostdin', '-loglevel', 'error', '-y', '-i', input, '-ar', '16000', '-ac', '1', '-c:a', 'pcm_s16le', wav], 60_000);
    if (conv.code !== 0) throw new HttpError(400, `Couldn’t decode the recording: ${lastLine(conv.err) || 'ffmpeg failed'}`);
    const threads = String(Math.max(1, Math.min(8, availableParallelism())));
    const out = join(dir, 'out');
    const r = await run(s.local.binary, ['-m', model, '-f', wav, '-l', s.language, '-t', threads, '-nt', '-np', '-otxt', '-of', out], 5 * 60_000);
    if (r.code !== 0 || !existsSync(`${out}.txt`)) throw new HttpError(500, `whisper.cpp failed: ${lastLine(r.err) || `exit code ${r.code}`}`);
    return cleanTranscript(readFileSync(`${out}.txt`, 'utf8'));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

const EXT_BY_MIME: Record<string, string> = {
  'audio/webm': 'webm',
  'audio/ogg': 'ogg',
  'audio/mp4': 'm4a',
  'audio/x-m4a': 'm4a',
  'audio/aac': 'm4a',
  'audio/mpeg': 'mp3',
  'audio/wav': 'wav',
  'audio/x-wav': 'wav',
  'audio/flac': 'flac',
  'video/webm': 'webm',
  'video/mp4': 'mp4',
};

/** File name for the multipart upload: services tell the format by its extension. */
export const audioFileName = (mime: string) => `recording.${EXT_BY_MIME[mime.split(';')[0]!.trim().toLowerCase()] ?? 'webm'}`;

export const transcriptionUrl = (url: string) => (/\/audio\/transcriptions\/?$/.test(url) ? url : `${url.replace(/\/+$/, '')}/audio/transcriptions`);

async function transcribeApi(audio: Uint8Array, mime: string, s: VoiceSettings): Promise<string> {
  if (!s.api.url || !s.api.model) throw new HttpError(409, 'The transcription service is not set up: Settings → Voice input');
  const blob = stored().apiKey;
  let key = '';
  if (blob) {
    try {
      key = decrypt(NS, 'apiKey', blob);
    } catch {
      throw new HttpError(409, 'The stored API key can’t be decrypted: enter it again in Settings → Voice input');
    }
  }
  const form = new FormData();
  form.append('file', new Blob([audio as Uint8Array<ArrayBuffer>], { type: mime.split(';')[0] || 'application/octet-stream' }), audioFileName(mime));
  form.append('model', s.api.model);
  if (s.language !== 'auto') form.append('language', s.language);
  let res: Response;
  try {
    res = await fetch(transcriptionUrl(s.api.url), {
      method: 'POST',
      headers: key ? { authorization: `Bearer ${key}` } : {},
      body: form,
      signal: AbortSignal.timeout(120_000),
    });
  } catch (e) {
    throw new HttpError(502, `The transcription service is unreachable: ${(e as Error).message}`);
  }
  const text = await res.text();
  let data: { text?: unknown; error?: { message?: unknown } | string } | null = null;
  try {
    data = JSON.parse(text);
  } catch {}
  if (!res.ok) {
    const err = data?.error;
    const msg = typeof err === 'string' ? err : typeof err?.message === 'string' ? err.message : text.slice(0, 300);
    throw new HttpError(502, `The transcription service answered ${res.status}: ${msg.trim() || res.statusText}`);
  }
  return (typeof data?.text === 'string' ? data.text : text).trim();
}

export async function transcribe(audio: Uint8Array, mime: string): Promise<{ text: string }> {
  if (!audio.length) throw new HttpError(400, 'The recording is empty');
  if (audio.length > MAX_AUDIO) throw new HttpError(413, `The recording is larger than ${MAX_AUDIO / 1024 / 1024} MB`);
  const s = voiceSettings();
  if (s.provider === 'off') throw new HttpError(409, 'Voice input is off: Settings → Voice input');
  const text = s.provider === 'local' ? await transcribeLocal(audio, s) : await transcribeApi(audio, mime, s);
  return { text };
}
