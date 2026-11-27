import './setup.ts';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { kv } from '../src/db/index.ts';
import { setVoiceSettings, voiceSettings } from '../src/voice.ts';

test('voice: a cloud service left in older settings counts as off, and its key goes with the next save', () => {
  kv.set('settings.voice', { provider: 'api', language: 'ru', apiUrl: 'https://api.groq.com/openai/v1', apiModel: 'whisper-large-v3-turbo', apiKey: 'sealed' });
  const s = voiceSettings();
  assert.deepEqual([s.provider, s.ready, s.language], ['off', false, 'ru']);
  assert.equal('api' in s, false);
  setVoiceSettings({ localModel: 'tiny' });
  assert.deepEqual(kv.get('settings.voice')?.value, { language: 'ru', localModel: 'tiny' }, 'no URL, model or encrypted key is kept');
});
