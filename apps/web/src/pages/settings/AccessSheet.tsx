import { useEffect, useState } from 'react';
import type { AgentInfo, Permissions, Project, Role, Section, User } from '@hubness/shared';
import { api } from '../../lib/api';
import { t } from '../../i18n';
import { Button, Checkbox, ErrorLine, Sheet, Spinner, Switch } from '../../components/ui';

const SECTIONS: Section[] = ['git', 'files', 'terminal', 'kanban', 'system', 'harness', 'settings'];
/** Sections that amount to a shell on the hub's machine. */
const HOST_SECTIONS: Section[] = ['terminal', 'system', 'harness'];
const MEMBER_DEFAULT: Permissions = { projects: [], sections: ['files', 'git'], agents: null, models: null };

const toggle = <T,>(list: T[], v: T, on: boolean) => (on ? (list.includes(v) ? list : [...list, v]) : list.filter((x) => x !== v));

/** A list with an "all" switch: `null` — everything, else the ticked items. */
function Pick<T extends string>({
  title,
  all,
  allHint,
  value,
  items,
  onChange,
}: {
  title: string;
  all: string;
  allHint?: string;
  value: T[] | null;
  items: { id: T; label: string; sub?: string }[] | null;
  onChange: (v: T[] | null) => void;
}) {
  return (
    <section className="space-y-2">
      <h3 className="text-sm font-semibold">{title}</h3>
      <Switch checked={value === null} onChange={(on) => onChange(on ? null : [])} label={all} hint={allHint} />
      {value === null ? null : items === null ? (
        <Spinner />
      ) : items.length ? (
        <ul className="max-h-56 space-y-1.5 overflow-y-auto border border-line bg-bg p-2">
          {items.map((i) => (
            <li key={i.id}>
              <Checkbox checked={value.includes(i.id)} onChange={(on) => onChange(toggle(value, i.id, on))}>
                <span className="text-sm">{i.label}</span>
                {i.sub ? <span className="ml-2 font-mono text-xs text-muted">{i.sub}</span> : null}
              </Checkbox>
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-xs text-muted">{t('users.nothingToPick')}</p>
      )}
    </section>
  );
}

/** An account's role and, for a member, what it may open. */
export function AccessSheet({ user, onClose, onSaved }: { user: User | null; onClose: () => void; onSaved: (u: User) => void }) {
  const [role, setRole] = useState<Role>('member');
  const [perms, setPerms] = useState<Permissions>(MEMBER_DEFAULT);
  const [projects, setProjects] = useState<Project[] | null>(null);
  const [agents, setAgents] = useState<AgentInfo[] | null>(null);
  const [models, setModels] = useState<Record<string, string[]>>({});
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!user) return;
    setRole(user.role);
    setPerms(user.permissions ?? MEMBER_DEFAULT);
    setError(null);
    api.projects().then(setProjects, () => setProjects([]));
    api.agents().then((r) => setAgents(r.agents), () => setAgents([]));
  }, [user]);

  // models of the agents the member may use (every agent when not limited)
  const kinds = (agents ?? []).map((a) => a.kind).filter((k) => perms.agents === null || perms.agents.includes(k));
  const kindsKey = kinds.join(',');
  useEffect(() => {
    if (perms.models === null) return;
    for (const k of kindsKey.split(',').filter(Boolean)) {
      if (models[k]) continue;
      api.agentModels(k as AgentInfo['kind']).then(
        (r) => setModels((m) => ({ ...m, [k]: r.models })),
        () => setModels((m) => ({ ...m, [k]: [] })),
      );
    }
  }, [kindsKey, perms.models === null]); // eslint-disable-line react-hooks/exhaustive-deps

  const modelItems = perms.models === null || kinds.some((k) => !models[k])
    ? null
    : kinds.flatMap((k) => (models[k] ?? []).map((m) => ({ id: m, label: m, sub: agents?.find((a) => a.kind === k)?.label })));
  // a model may be listed by several agents
  const uniqueModels = modelItems?.filter((m, i, all) => all.findIndex((x) => x.id === m.id) === i) ?? null;

  const save = async () => {
    if (!user) return;
    setBusy(true);
    setError(null);
    try {
      onSaved(await api.setUserAccess(user.id, role === 'admin' ? { role } : { role, permissions: perms }));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const set = (p: Partial<Permissions>) => setPerms((cur) => ({ ...cur, ...p }));
  const host = perms.sections.some((s) => HOST_SECTIONS.includes(s));

  return (
    <Sheet open={user !== null} onClose={onClose} title={user ? t('users.accessFor', { name: user.username }) : ''}>
      <div className="space-y-5">
        <section className="space-y-2">
          <h3 className="text-sm font-semibold">{t('users.role')}</h3>
          <div className="grid grid-cols-2 border border-line">
            {(['admin', 'member'] as const).map((r) => (
              <button
                key={r}
                type="button"
                onClick={() => setRole(r)}
                aria-pressed={role === r}
                className={`px-3 py-2 text-sm ${role === r ? 'bg-sunken text-accent' : 'text-muted hover:text-fg'}`}
              >
                {t(r === 'admin' ? 'users.role.admin' : 'users.role.member')}
              </button>
            ))}
          </div>
          <p className="text-xs text-muted">{t(role === 'admin' ? 'users.roleAdminHint' : 'users.roleMemberHint')}</p>
        </section>

        {role === 'member' ? (
          <>
            <Pick
              title={t('users.projects')}
              all={t('users.allProjects')}
              allHint={t('users.allProjectsHint')}
              value={perms.projects}
              items={projects?.map((p) => ({ id: p.id, label: p.name })) ?? null}
              onChange={(projects) => set({ projects })}
            />
            <section className="space-y-2">
              <h3 className="text-sm font-semibold">{t('users.sections')}</h3>
              <ul className="space-y-1.5">
                {SECTIONS.map((s) => (
                  <li key={s}>
                    <Checkbox checked={perms.sections.includes(s)} onChange={(on) => set({ sections: toggle(perms.sections, s, on) })}>
                      <span className="text-sm">{t(`users.section.${s}` as const)}</span>
                    </Checkbox>
                  </li>
                ))}
              </ul>
              {host ? <p className="text-xs text-warn">{t('users.hostWarning')}</p> : null}
            </section>
            <Pick
              title={t('users.agents')}
              all={t('users.allAgents')}
              value={perms.agents}
              items={agents?.map((a) => ({ id: a.kind, label: a.label })) ?? null}
              onChange={(agents) => set({ agents })}
            />
            <Pick title={t('users.models')} all={t('users.allModels')} value={perms.models} items={uniqueModels} onChange={(models) => set({ models })} />
          </>
        ) : null}

        <ErrorLine error={error} />
        <Button variant="primary" loading={busy} onClick={() => void save()}>
          {t('common.save')}
        </Button>
      </div>
    </Sheet>
  );
}
