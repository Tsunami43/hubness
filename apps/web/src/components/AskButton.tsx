import { useNavigate } from 'react-router';
import { MessageCircleQuestion } from 'lucide-react';
import type { AskTarget } from '@hubness/shared';
import { askAbout, setAskDraft } from '../lib/askDraft';
import { t } from '../i18n';
import { Button } from './ui';

/**
 * "Ask about this": opens the start page with the object's name already in the input and
 * the system project picked — nothing runs until the user writes the question and sends.
 * `text` overrides the prefilled text (e.g. the logs from the logs sheet).
 */
export function AskButton({ target, text, label, size = 'sm', variant = 'secondary' }: {
  target?: AskTarget;
  text?: string;
  label?: string;
  size?: 'sm' | 'md';
  variant?: 'primary' | 'secondary' | 'ghost';
}) {
  const nav = useNavigate();
  return (
    <Button
      size={size}
      variant={variant}
      title={t('ask.title')}
      onClick={(e) => {
        e.stopPropagation();
        setAskDraft(text ?? (target ? askAbout(target) : ''));
        nav('/');
      }}
    >
      <MessageCircleQuestion className="size-4" />
      {label ?? t('ask.label')}
    </Button>
  );
}
