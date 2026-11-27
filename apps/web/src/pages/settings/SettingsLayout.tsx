import { Bot, GitBranch, HardDrive, Keyboard, Languages, Mic, Palette, Users } from 'lucide-react';
import { SubmenuIndex, SubmenuLayout, type SubmenuItem } from '../../components/SubmenuLayout';
import { t } from '../../i18n';
import { useAccess } from '../../components/AuthGate';

type Item = SubmenuItem & { need?: 'settings' | 'admin' };

const items = (): Item[] => [
  {
    to: 'agent',
    need: 'settings',
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
    need: 'settings',
    label: t('voice.title'),
    hint: t('voice.itemHint'),
    icon: <Mic className="size-4 text-accent" />,
    keywords: ['voice', 'speech', 'microphone', 'dictation', 'whisper', 'transcription', 'голос', 'речь', 'микрофон', 'диктовка', 'распознавание', 'голосовой'],
  },
  {
    to: 'git',
    need: 'settings',
    label: t('gitauth.title'),
    hint: t('gitauth.itemHint'),
    icon: <GitBranch className="size-4 text-accent" />,
    keywords: ['git', 'login', 'password', 'token', 'https', 'auth', 'github', 'gitlab', 'credentials', 'доступ', 'вход', 'логин', 'пароль', 'токен', 'авторизация'],
  },
  {
    to: 'users',
    label: t('users.title'),
    hint: t('users.itemHint'),
    icon: <Users className="size-4 text-accent" />,
    keywords: ['users', 'accounts', 'password', 'login', 'logout', 'sign in', 'пользователи', 'учётные записи', 'пароль', 'вход', 'выход'],
  },
  {
    to: 'storage',
    need: 'admin',
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
    hint: t('settings.languageItemHint'),
    icon: <Languages className="size-4 text-accent" />,
    keywords: ['language', 'english', 'russian', 'locale', 'язык', 'русский', 'английский'],
  },
];

/** Settings: submenu with search. */
/** The settings this account may open (members: hub-wide ones only with the settings section). */
function useItems(): SubmenuItem[] {
  const access = useAccess();
  return items()
    .filter((i) => !i.need || (i.need === 'admin' ? access.admin : access.can(i.need)))
    .map(({ need: _, ...i }) => i);
}

export function SettingsLayout() {
  return <SubmenuLayout base="/settings" title={t('settings.title')} items={useItems()} searchPlaceholder={t('settings.search')} />;
}

export function SettingsIndex() {
  return <SubmenuIndex to={useItems()[0]?.to} />;
}
