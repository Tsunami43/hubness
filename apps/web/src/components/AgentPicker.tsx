import { useEffect, useState } from 'react';
import type { AgentConfig, AgentKind, ModelOption } from '@hubness/shared';
import { api } from '../lib/api';
import { useAgents } from '../lib/agents';
import { PERMISSION_MODES, permissionHint, permissionLabel } from '../lib/format';
import { t, tOptional } from '../i18n';
import { AgentIcon } from './AgentIcon';
import { ModelPicker } from './ModelPicker';

// models per agent, kept for the page lifetime (the CLI lists are slow to read)
const modelsCache = new Map<AgentKind, { models: string[]; options: ModelOption[] }>();

/** Pick the agent (harness), model and permission mode. lockKind is for an already created session. */
export function AgentPicker({
  value,
  onChange,
  lockKind,
}: {
  value: AgentConfig;
  onChange: (a: AgentConfig) => void;
  lockKind?: boolean;
}) {
  const agents = useAgents();
  const [models, setModels] = useState(modelsCache.get(value.kind) ?? { models: [], options: [] });
  const [loadingModels, setLoadingModels] = useState(false);

  useEffect(() => {
    const kind = value.kind;
    const cached = modelsCache.get(kind);
    setModels(cached ?? { models: [], options: [] });
    if (cached?.models.length) return;
    setLoadingModels(true);
    api
      .agentModels(kind)
      .then(
        (r) => {
          const m = { models: r.models, options: r.options ?? [] };
          modelsCache.set(kind, m);
          setModels(m);
        },
        () => {},
      )
      .finally(() => setLoadingModels(false));
  }, [value.kind]);

  const current = agents?.agents.find((a) => a.kind === value.kind);
  const kind = value.kind;

  return (
    <div className="space-y-4">
      {!lockKind ? (
        <div className="space-y-2">
          {(agents?.agents ?? []).map((a) => {
            const disabled = !a.available;
            return (
              <button
                key={a.kind}
                type="button"
                disabled={disabled}
                onClick={() => onChange({ kind: a.kind, permissionMode: value.permissionMode ?? 'ask' })}
                className={`block w-full rounded-xl border px-3.5 py-2.5 text-left transition disabled:opacity-45 ${
                  value.kind === a.kind ? 'border-accent bg-accent/8 ring-1 ring-accent' : 'border-line bg-surface hover:bg-sunken'
                }`}
              >
                <div className="flex items-center gap-2">
                  <AgentIcon kind={a.kind} />
                  <span className="font-medium">{a.label}</span>
                  {a.available && a.version ? <span className="font-mono text-xs text-muted">v{a.version}</span> : null}
                </div>
                <div className="mt-0.5 pl-6 text-sm text-muted">{disabled ? a.detail : (tOptional(`agent.hint.${a.kind}`) ?? a.detail)}</div>
              </button>
            );
          })}
        </div>
      ) : null}

      <div>
        <div className="mb-1.5 text-sm font-medium">{t('agent.model')}</div>
        {current?.modelStyle === 'options' ? (
          <div className="flex flex-wrap gap-1.5">
            {[{ id: '', label: t('agent.modelDefault') } as ModelOption, ...models.options].map((m) => (
              <button
                key={m.id}
                type="button"
                title={m.description}
                onClick={() => onChange({ ...value, model: m.id || undefined })}
                className={`rounded-full border px-3 py-1.5 text-sm ${(value.model ?? '') === m.id ? 'btn-accent' : 'border-line bg-surface hover:bg-sunken'}`}
              >
                {m.label}
              </button>
            ))}
            {loadingModels ? <span className="self-center text-xs text-muted">…</span> : null}
          </div>
        ) : (
          <>
            <ModelPicker
              models={models.models}
              value={value.model}
              loading={loadingModels}
              defaultLabel={tOptional(`agent.modelPlaceholder.${kind}`) ?? t('agent.modelPlaceholder')}
              onChange={(m) => onChange({ ...value, model: m })}
            />
            <div className="mt-1 text-xs text-muted">{tOptional(`agent.modelHint.${kind}`) ?? t('agent.modelHint')}</div>
          </>
        )}
      </div>

      <div>
        <div className="mb-1.5 text-sm font-medium">{t('agent.permissions')}</div>
        <div className="grid grid-cols-2 gap-1.5">
          {PERMISSION_MODES.map((m) => (
            <button
              key={m}
              type="button"
              onClick={() => onChange({ ...value, permissionMode: m })}
              className={`rounded-lg border px-2.5 py-2 text-left ${
                (value.permissionMode ?? 'ask') === m ? 'border-accent ring-1 ring-accent' : 'border-line bg-surface hover:bg-sunken'
              }`}
            >
              <div className={`text-sm font-medium ${m === 'bypass' ? 'text-warn' : ''}`}>{permissionLabel(m)}</div>
              <div className="text-xs leading-snug text-muted">{permissionHint(m)}</div>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
