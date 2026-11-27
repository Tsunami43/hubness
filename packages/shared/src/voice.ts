// Voice input: speech to text for the chat input (Settings → Voice input).

/**
 * Where speech is transcribed: off, a local whisper.cpp run by the hub, or any
 * OpenAI-compatible `/audio/transcriptions` endpoint (OpenAI, Groq, speaches, whisper-server…).
 */
export type VoiceProvider = 'off' | 'local' | 'api';

/** A whisper.cpp (ggml) model the hub can download. */
export interface VoiceModel {
  id: string;
  label: string;
  /** Download size in bytes (approximate). */
  size: number;
  installed: boolean;
}

export interface VoiceSettings {
  provider: VoiceProvider;
  /** `auto` or an ISO 639-1 code (`en`, `ru`…). */
  language: string;
  /** The selected provider is set up: the composer shows the microphone. */
  ready: boolean;
  local: {
    /** Selected model id. */
    model: string;
    /** whisper-cli the hub runs: its own build or one on PATH; null — not found. */
    binary: string | null;
    /** The binary is the hub's own build (in the data folder). */
    managed: boolean;
    /** ffmpeg converts the browser recording to 16 kHz WAV for whisper.cpp. */
    ffmpeg: boolean;
    /** Tools missing to build whisper.cpp from source (git, cmake, a C++ compiler). */
    missingBuildTools: string[];
    models: VoiceModel[];
  };
  api: {
    /** Base URL (`https://api.groq.com/openai/v1`) or the full `/audio/transcriptions` URL. */
    url: string;
    model: string;
    /** A key is stored (the key itself never leaves the server). */
    hasKey: boolean;
  };
}

/** PUT /api/voice: only the given fields change; `apiKey: null` removes the stored key. */
export interface VoiceSettingsInput {
  provider?: VoiceProvider;
  language?: string;
  localModel?: string;
  apiUrl?: string;
  apiModel?: string;
  apiKey?: string | null;
}

export interface TranscriptionResult {
  text: string;
}
