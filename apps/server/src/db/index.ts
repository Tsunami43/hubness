import { db } from './connection.ts';
import { migrate } from './migrations.ts';
import { recoverInterruptedSessions } from './sessions.ts';

/**
 * Data access, one module per table group. Importing this module opens the database,
 * applies pending migrations and resets sessions cut off by a restart.
 */

migrate(db);
recoverInterruptedSessions();

export { db, tx } from './connection.ts';
export { cards } from './cards.ts';
export { gitCredentialRows, secretRows, type GitCredentialRow } from './credentials.ts';
export { groups } from './groups.ts';
export { DEFAULT_INSTRUCTIONS_KEY, SESSION_INSTRUCTIONS_KEY, kv } from './kv.ts';
export { projectRuntime, projects } from './projects.ts';
export { runConfigs } from './runs.ts';
export { events, sessions } from './sessions.ts';
export { workspaces } from './workspaces.ts';
