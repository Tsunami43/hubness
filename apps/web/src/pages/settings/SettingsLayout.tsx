import { Bot, HardDrive, Keyboard, Languages, Mic, Palette, UserRound } from 'lucide-react';
import { SubmenuIndex, SubmenuLayout, type SubmenuItem } from '../../components/SubmenuLayout';
import { getLang, LANGUAGES, t } from '../../i18n';

const items = (): SubmenuItem[] => [
  {
    to: 'agent',
    label: t('settings.agent'),
    hint: t('settings.agentHint'),
    icon: <Bot className="size-4 text-accent" />,
    keywords: ['agent', 'permissions', 'approvals', 'ask', 'auto', 'bypass', 'plan', 'агент', 'разрешения', 'права', 'instructions', 'prompt', 'инструкции', 'промпт'],
  },
  {
    to: 'appearance',
    label: t('settings.appearance'),
    hint: t('settings.appearanceHint'),
    icon: <Palette className="size-4 text-accent" />,
    keywords: ['theme', 'standard', 'colors', 'look', 'design', 'dark', 'editor', 'vim', 'font', 'size', 'тема', 'оформление', 'редактор', 'шрифт', 'размер'],
  },
  {
    to: 'voice',
    label: t('voice.title'),
    hint: t('voice.itemHint'),
    icon: <Mic className="size-4 text-accent" />,
    keywords: ['voice', 'speech', 'microphone', 'dictation', 'whisper', 'transcription', 'голос', 'речь', 'микрофон', 'диктовка', 'распознавание', 'голосовой'],
  },
  {
    to: 'account',
    label: t('account.title'),
    hint: t('account.itemHint'),
    icon: <UserRound className="size-4 text-accent" />,
    keywords: ['account', 'user', 'password', 'login', 'logout', 'sign in', 'sign out', 'аккаунт', 'учётная запись', 'пароль', 'вход', 'выход', 'выйти'],
  },
  {
    to: 'storage',
    label: t('storage.title'),
    hint: t('storage.itemHint'),
    icon: <HardDrive className="size-4 text-accent" />,
    keywords: ['storage', 'data', 'folder', 'directory', 'database', 'disk', 'хранилище', 'данные', 'папка', 'база', 'диск'],
  },
  {
    to: 'keyboard',
    label: t('settings.keyboard'),
    hint: t('settings.keyboardHint'),
    icon: <Keyboard className="size-4 text-accent" />,
    keywords: ['keyboard', 'shortcuts', 'hotkeys', 'leader', 'vim', 'клавиши', 'горячие', 'шорткаты'],
  },
  {
    to: 'language',
    label: t('settings.language'),
    hint: LANGUAGES.find((l) => l.id === getLang())?.label,
    icon: <Languages className="size-4 text-accent" />,
    keywords: ['language', 'locale', 'translation', 'язык', 'перевод', ...LANGUAGES.map((l) => l.label.toLowerCase())],
  },
];

/** Settings: submenu with search. */
export function SettingsLayout() {
  return <SubmenuLayout base="/settings" title={t('settings.title')} items={items()} searchPlaceholder={t('settings.search')} />;
}

export function SettingsIndex() {
  return <SubmenuIndex to={items()[0]?.to} />;
}
