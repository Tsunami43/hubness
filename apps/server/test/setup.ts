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
// per-turn ssh-agent sockets (ssh.ts) go here, not into the real runtime folder
process.env.XDG_RUNTIME_DIR = join(tmp, 'run');
delete process.env.HUBNESS_SECRET_KEY;
// the agents' folders (~/.claude, ~/.codex…) of a temporary HOME: building sandbox mounts
// creates their runtime folders, never in the real one
process.env.HOME = join(tmp, 'home');
mkdirSync(process.env.HOME);
delete process.env.CODEX_HOME;
delete process.env.GROK_HOME;

/** Create a git repository (optionally with a remote origin). */
export function makeRepo(path: string, remote?: string): string {
  mkdirSync(path, { recursive: true });
  execFileSync('git', ['init', '-q', '-b', 'main', path]);
  if (remote) execFileSync('git', ['-C', path, 'remote', 'add', 'origin', remote]);
  return path;
}
