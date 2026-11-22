import './setup.ts';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { DatabaseSync } from 'node:sqlite';
import { MIGRATIONS, migrate, schemaVersion, type Migration } from '../src/db/migrations.ts';

const latest = Math.max(...MIGRATIONS.map((m) => m.version));

test('a fresh database is migrated to the latest version, and a second run does nothing', () => {
  const db = new DatabaseSync(':memory:');
  assert.equal(migrate(db).length, MIGRATIONS.length);
  assert.equal(schemaVersion(db), latest);
  assert.deepEqual(migrate(db), []);
  const tables = db.prepare("SELECT name FROM sqlite_master WHERE type = 'table'").all().map((r) => r.name);
  for (const t of ['projects', 'sessions', 'events', 'project_groups', 'group_members', 'kv']) assert.ok(tables.includes(t), t);
});

test('a database from before versioning (user_version 0, old columns) is upgraded in place', () => {
  const db = new DatabaseSync(':memory:');
  // an early schema: projects without the later columns, one row in it
  db.exec(`CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL UNIQUE, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
           CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at TEXT NOT NULL);
           INSERT INTO kv VALUES ('migration.explicitProjects', 'true', '2026-01-01');
           INSERT INTO projects VALUES ('p1', 'kept', '/repos/kept', '2026-01-01', '2026-01-01');`);
  migrate(db);
  const cols = db.prepare('PRAGMA table_info(projects)').all().map((r) => r.name);
  for (const c of ['kind', 'description', 'instructions', 'runtime', 'language']) assert.ok(cols.includes(c), c);
  assert.equal(db.prepare("SELECT kind FROM projects WHERE id = 'p1'").get()?.kind, 'normal');
  assert.equal(schemaVersion(db), latest);
});

test('a failing migration rolls back and leaves the previous version', () => {
  const db = new DatabaseSync(':memory:');
  migrate(db);
  const broken: Migration = {
    version: latest + 1,
    name: 'broken',
    up(d) {
      d.exec('CREATE TABLE half_done (x INTEGER)');
      throw new Error('boom');
    },
  };
  assert.throws(() => migrate(db, [...MIGRATIONS, broken]), /Migration .* \(broken\) failed: boom/);
  assert.equal(schemaVersion(db), latest);
  assert.equal(db.prepare("SELECT name FROM sqlite_master WHERE name = 'half_done'").get(), undefined);
});
