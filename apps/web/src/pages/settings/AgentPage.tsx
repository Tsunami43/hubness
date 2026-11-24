import { useState } from 'react';
import { Check, TriangleAlert } from 'lucide-react';
import type { PermissionMode } from '@hubness/shared';
import { api } from '../../lib/api';
import { PERMISSION_MODES, permissionHint, permissionLabel } from '../../lib/format';
import { publish, useHubSettings } from '../../lib/hubSettings';
import { Button, ErrorLine, inputCls, Spinner } from '../../components/ui';
import { t } from '../../i18n';

/**
 * Settings → Agent: the permission mode every new session starts with (still changeable per
 * session), instructions added to every session, and the instructions every new project starts with. Stored on the server, so they
 * are the same on every device.
 */
export function AgentPage() {
  const hub = useHubSettings();
  const [busy, setBusy] = useState<PermissionMode | null>(null);
  const [error, setError] = useState<string | null>(null);
  if (!hub) return <Spinner />;

  const pick = async (m: PermissionMode) => {
    if (m === hub.defaultPermission) return;
    setBusy(m);
    setError(null);
    try {
      publish(await api.saveSettings({ defaultPermission: m }));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="space-y-6">
      <section>
        <h2 className="mb-1 font-semibold">{t('agentSettings.permission')}</h2>
        <p className="mb-3 text-xs text-muted">{t('agentSettings.permissionHint')}</p>
        <ul className="divide-y divide-line border border-line bg-surface">
          {PERMISSION_MODES.map((m) => (
            <li key={m}>
              <button type="button" onClick={() => void pick(m)} className="flex w-full items-start gap-3 px-4 py-3 text-left hover:bg-sunken">
                <span className="min-w-0 flex-1">
                  <span className={`block font-medium ${m === 'bypass' ? 'text-warn' : ''}`}>
                    {permissionLabel(m)}
                    {m === 'ask' ? <span className="ml-2 text-xs font-normal text-muted">{t('agentSettings.recommended')}</span> : null}
                  </span>
                  <span className="block text-xs text-muted">{permissionHint(m)}</span>
                  <span className="mt-1 block text-xs text-muted/80">{t(`agentSettings.${m}`)}</span>
                </span>
                {busy === m ? <Spinner className="size-4" /> : m === hub.defaultPermission ? <Check className="mt-0.5 size-5 shrink-0 text-accent" /> : null}
              </button>
            </li>
          ))}
        </ul>
        {hub.defaultPermission === 'bypass' ? (
          <p className="mt-2 flex items-start gap-2 border border-warn/40 bg-warn/10 px-3 py-2 text-xs">
            <TriangleAlert className="mt-0.5 size-3.5 shrink-0 text-warn" />
            {t('agentSettings.bypassWarn')}
          </p>
        ) : null}
        <ErrorLine error={error} />
      </section>
      <InstructionsField
        field="sessionInstructions"
        initial={hub.sessionInstructions}
        title={t('agentSettings.sessionInstructions')}
        hint={t('agentSettings.sessionInstructionsHint')}
        placeholder={t('agentSettings.sessionInstructionsPlaceholder')}
      />
      <InstructionsField
        field="defaultInstructions"
        initial={hub.defaultInstructions}
        title={t('agentSettings.instructions')}
        hint={t('agentSettings.instructionsHint')}
        placeholder={t('agentSettings.instructionsPlaceholder')}
      />
    </div>
  );
}

/**
 * A hub-wide instructions text: `sessionInstructions` are added to every session live,
 * `defaultInstructions` are copied into each new project (editable there).
 */
function InstructionsField({
  field,
  initial,
  title,
  hint,
  placeholder,
}: {
  field: 'sessionInstructions' | 'defaultInstructions';
  initial: string;
  title: string;
  hint: string;
  placeholder: string;
}) {
  const [text, setText] = useState(initial);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dirty = text.trim() !== initial.trim();
  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      publish(await api.saveSettings({ [field]: text }));
      setSaved(true);
      setTimeout(() => setSaved(false), 1500);
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <section>
      <h2 className="mb-1 font-semibold">{title}</h2>
      <p className="mb-3 text-xs text-muted">{hint}</p>
      <textarea
        rows={6}
        className={inputCls}
        value={text}
        onChange={(e) => setText(e.target.value)}
        placeholder={placeholder}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && (e.metaKey || e.ctrlKey) && dirty) void save();
        }}
      />
      <div className="mt-2 flex items-center gap-3">
        <Button size="sm" variant="primary" disabled={!dirty || busy} loading={busy} onClick={() => void save()}>
          {t('common.save')}
        </Button>
        {saved ? (
          <span className="inline-flex items-center gap-1 text-xs text-ok">
            <Check className="size-3.5" /> {t('agentSettings.saved')}
          </span>
        ) : null}
      </div>
      <ErrorLine error={error} />
    </section>
  );
}
