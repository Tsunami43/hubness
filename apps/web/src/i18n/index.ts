import { en, type MessageKey } from './en';

type Dict = Partial<Record<MessageKey, string>>;

/**
 * Available UI languages, `label` is the language's own name. English is built in; every
 * other dictionary is its own chunk, loaded only when chosen. To offer another language,
 * add a dictionary file and a line here.
 */
export const LANGUAGES = [
  { id: 'en', label: 'English', load: async (): Promise<Dict> => en },
  { id: 'ru', label: 'Русский', load: () => import('./ru').then((m) => m.ru) },
  { id: 'uk', label: 'Українська', load: () => import('./uk').then((m) => m.uk) },
  { id: 'de', label: 'Deutsch', load: () => import('./de').then((m) => m.de) },
  { id: 'fr', label: 'Français', load: () => import('./fr').then((m) => m.fr) },
  { id: 'es', label: 'Español', load: () => import('./es').then((m) => m.es) },
  { id: 'it', label: 'Italiano', load: () => import('./it').then((m) => m.it) },
  { id: 'pt', label: 'Português', load: () => import('./pt').then((m) => m.pt) },
  { id: 'pl', label: 'Polski', load: () => import('./pl').then((m) => m.pl) },
  { id: 'tr', label: 'Türkçe', load: () => import('./tr').then((m) => m.tr) },
  { id: 'zh', label: '简体中文', load: () => import('./zh').then((m) => m.zh) },
  { id: 'ja', label: '日本語', load: () => import('./ja').then((m) => m.ja) },
  { id: 'ko', label: '한국어', load: () => import('./ko').then((m) => m.ko) },
] as const;

export type Lang = (typeof LANGUAGES)[number]['id'];

const STORAGE_KEY = 'hub.lang';
const known = (id: string | null | undefined): id is Lang => LANGUAGES.some((l) => l.id === id);

function initialLang(): Lang {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (known(saved)) return saved;
  } catch {}
  // not chosen — use the browser language if a dictionary exists for it
  const browser = navigator.language?.slice(0, 2);
  return known(browser) ? browser : 'en';
}

let current: Lang = initialLang();
let dict: Dict;
try {
  // top-level await: modules that import i18n run once the dictionary is in
  dict = await LANGUAGES.find((l) => l.id === current)!.load();
} catch {
  current = 'en';
  dict = en;
}
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
