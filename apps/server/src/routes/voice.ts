import { Hono } from 'hono';
import { z } from 'zod';
import type { VoiceSettingsInput } from '@hubness/shared';
import { HttpError } from '../errors.ts';
import { deleteModel, downloadModel, installWhisper, MAX_AUDIO, setVoiceSettings, transcribe, voiceSettings } from '../voice.ts';
import { parseBody, TermSizeSchema } from './http.ts';

/** Voice input: settings, whisper.cpp build and models, transcription of a recording. */
export const voiceRoutes = new Hono();

// field values are checked by voice.ts with the same messages as the Rust server
const Settings = z.object({
  provider: z.unknown().optional(),
  language: z.unknown().optional(),
  localModel: z.unknown().optional(),
  apiUrl: z.unknown().optional(),
  apiModel: z.unknown().optional(),
  apiKey: z.unknown().optional(),
});

voiceRoutes.get('/voice', (c) => c.json(voiceSettings()));
voiceRoutes.put('/voice', async (c) => c.json(setVoiceSettings((await parseBody(c, Settings)) as VoiceSettingsInput)));
/** Build whisper.cpp from source: a terminal job (/api/jobs/:id/stream). */
voiceRoutes.post('/voice/install', async (c) => c.json(installWhisper(await parseBody(c, TermSizeSchema, { lenient: true })), 201));
/** Download a model: a terminal job. */
voiceRoutes.post('/voice/models/:id', async (c) => {
  const size = await parseBody(c, TermSizeSchema, { lenient: true });
  return c.json(downloadModel(c.req.param('id'), size), 201);
});
voiceRoutes.delete('/voice/models/:id', (c) => c.json(deleteModel(c.req.param('id'))));
/** The recording is the raw body (Content-Type: its audio type); answers `{ text }`. */
voiceRoutes.post('/voice/transcribe', async (c) => {
  if (Number(c.req.header('content-length') ?? 0) > MAX_AUDIO) throw new HttpError(413, `The recording is larger than ${MAX_AUDIO / 1024 / 1024} MB`);
  const audio = new Uint8Array(await c.req.arrayBuffer());
  return c.json(await transcribe(audio, c.req.header('content-type') ?? ''));
});
