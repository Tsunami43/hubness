import { en, type MessageKey } from './en';
import { ru } from './ru';

/** Available UI languages. Add a dictionary here to offer another one. */
export const LANGUAGES = [
  { id: 'en', label: 'English', dict: en },
  { id: 'ru', label: 'Русский', dict: ru },
] as const;

export type Lang = (typeof LANGUAGES)[number]['id'];

const STORAGE_KEY = 'hub.lang';

function initialLang(): Lang {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (LANGUAGES.some((l) => l.id === saved)) return saved as Lang;
  } catch {}
  // not chosen — use the browser language if a dictionary exists for it
  const browser = navigator.language?.slice(0, 2);
  return LANGUAGES.some((l) => l.id === browser) ? (browser as Lang) : 'en';
}

let current: Lang = initialLang();
let dict: Record<string, string> = LANGUAGES.find((l) => l.id === current)!.dict as Record<string, string>;
document.documentElement.lang = current;

export const getLang = () => current;

/** Switch language; the page reloads so every screen picks it up. */
export function setLang(lang: Lang) {
  if (lang === current) return;
  try {
    localStorage.setItem(STORAGE_KEY, lang);
  } catch {}
  location.reload();
}

/** Translate a key; {placeholders} are filled from params. */
export function t(key: MessageKey, params?: Record<string, string | number>): string {
  const s = dict[key] ?? en[key] ?? key;
  return params ? s.replace(/\{(\w+)\}/g, (_, k) => String(params[k] ?? `{${k}}`)) : s;
}

/**
 * Translate a key built at runtime (e.g. per agent kind: `agent.hint.<kind>`);
 * null when no dictionary has it, so the caller can fall back to a generic string.
 */
export function tOptional(key: string, params?: Record<string, string | number>): string | null {
  return key in dict || key in en ? t(key as MessageKey, params) : null;
}
