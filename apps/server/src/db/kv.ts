import { db, now } from './connection.ts';

// ---------- kv: small internal state (latest subscription limits, etc.) ----------

/** Settings → Agent: instructions every new project starts with. */
export const DEFAULT_INSTRUCTIONS_KEY = 'settings.defaultInstructions';
/** Settings → Agent: instructions added to every session (project, group, chat). */
export const SESSION_INSTRUCTIONS_KEY = 'settings.sessionInstructions';

export const kv = {
  get<T>(key: string): { value: T; updatedAt: string } | undefined {
    const r = db.prepare('SELECT value, updated_at FROM kv WHERE key = ?').get(key);
    return r ? { value: JSON.parse(r.value as string) as T, updatedAt: r.updated_at as string } : undefined;
  },
  set(key: string, value: unknown) {
    db.prepare(
      `INSERT INTO kv (key, value, updated_at) VALUES (?, ?, ?)
       ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at`,
    ).run(key, JSON.stringify(value), now());
  },
};
