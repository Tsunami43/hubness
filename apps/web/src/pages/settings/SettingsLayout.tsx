import { Bot, GitBranch, Keyboard, Languages, Palette, Users } from 'lucide-react';
import { SubmenuIndex, SubmenuLayout, type SubmenuItem } from '../../components/SubmenuLayout';
import { t } from '../../i18n';

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
    to: 'git',
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
export function SettingsLayout() {
  return <SubmenuLayout base="/settings" title={t('settings.title')} items={items()} searchPlaceholder={t('settings.search')} />;
}

export function SettingsIndex() {
  return <SubmenuIndex to="agent" />;
}
