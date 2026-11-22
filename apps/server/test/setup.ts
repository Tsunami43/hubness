import { mkdirSync, mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';

/**
 * Test isolation: own data folder and hub working folder in a temporary
 * directory. Imported FIRST in every test file — before the server
 * modules (config.ts reads variables at load time).
 */
export const tmp = mkdtempSync(join(tmpdir(), 'hubness-test-'));
process.env.HUBNESS_DATA_DIR = join(tmp, 'data');
process.env.HUBNESS_HOME = join(tmp, 'hub');
process.env.HUBNESS_PORT = '0';
process.env.HUBNESS_PREVIEW_PORTS = '18790-18799';
delete process.env.HUBNESS_SECRET_KEY;

/** Create a git repository (optionally with a remote origin). */
export function makeRepo(path: string, remote?: string): string {
  mkdirSync(path, { recursive: true });
  execFileSync('git', ['init', '-q', '-b', 'main', path]);
  if (remote) execFileSync('git', ['-C', path, 'remote', 'add', 'origin', remote]);
  return path;
}
