import { DatabaseSync } from 'node:sqlite';
import { config } from '../config.ts';

/** The hub database (SQLite, one file in the data folder). Schema changes go through migrations.ts. */
export const db = new DatabaseSync(config.dbPath);
db.exec('PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;');

export const now = () => new Date().toISOString();
export type Row = Record<string, unknown>;

/** Run fn in a transaction: committed if it returns, rolled back if it throws. */
export function tx<T>(fn: () => T): T {
  db.exec('BEGIN');
  try {
    const r = fn();
    db.exec('COMMIT');
    return r;
  } catch (e) {
    db.exec('ROLLBACK');
    throw e;
  }
}
