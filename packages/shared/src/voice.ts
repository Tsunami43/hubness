// Voice input: speech to text for the chat input (Settings → Voice input).

/** Where speech is transcribed: off, or a local whisper.cpp run by the hub. */
export type VoiceProvider = 'off' | 'local';

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
}

/** PUT /api/voice: only the given fields change. */
export interface VoiceSettingsInput {
  provider?: VoiceProvider;
  language?: string;
  localModel?: string;
}

export interface TranscriptionResult {
  text: string;
}
