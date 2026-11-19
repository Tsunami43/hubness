import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { parse, stringify } from 'yaml';
import type { Manifest, ManifestResponse, Project } from '@hubness/shared';
import { dockerContainers } from './system/monitor.ts';

/**
 * Project manifest .hub/hub.yaml — shared memory of the human and the agent:
 * links (Grafana, staging), SSH hosts, containers, units, documents.
 * The UI builds the Overview tab from it; the agent receives it as context.
 */

export const manifestPath = (p: Project) => join(p.path, '.hub', 'hub.yaml');

const strList = (v: unknown): string[] => (Array.isArray(v) ? v.map(String).filter(Boolean) : typeof v === 'string' ? [v] : []);

function normalize(raw: any): Manifest {
  const links: Record<string, string> = {};
  if (raw?.links && typeof raw.links === 'object') for (const [k, v] of Object.entries(raw.links)) links[k] = String(v);
  return {
    links,
    ssh: strList(raw?.ssh),
    services: strList(raw?.services),
    units: strList(raw?.units),
    docs: strList(raw?.docs),
    notes: typeof raw?.notes === 'string' ? raw.notes : undefined,
  };
}

export function readManifest(p: Project): { manifest: Manifest | null; error?: string; exists: boolean } {
  const path = manifestPath(p);
  if (!existsSync(path)) return { manifest: null, exists: false };
  try {
    return { manifest: normalize(parse(readFileSync(path, 'utf8')) ?? {}), exists: true };
  } catch (e) {
    return { manifest: null, exists: true, error: (e as Error).message };
  }
}

export async function manifestResponse(p: Project): Promise<ManifestResponse> {
  const { manifest, error, exists } = readManifest(p);
  let services: ManifestResponse['services'] = [];
  if (manifest?.services.length) {
    const containers = (await dockerContainers()) ?? [];
    services = manifest.services.map((name) => {
      const c = containers.find((x) => x.name === name);
      return { name, state: c?.state ?? 'missing', status: c?.status ?? 'not found' };
    });
  }
  return { exists, path: manifestPath(p), manifest, error, services };
}

const TEMPLATE = (name: string) => `# hubness project manifest — shared memory for you and the agents.
# The hub builds the Overview tab from it; agents get it as context.
links:
  # grafana: http://localhost:3000/d/${name}
  # staging: http://localhost:3001
ssh: []        # hosts from ~/.ssh/config, e.g. [prod, vps-proxy]
services: []   # docker containers of this project
units: []      # systemd units of this project
docs:
  - AGENTS.md
notes: |
  Anything agents should always know about this project.
`;

export function createManifest(p: Project): string {
  const path = manifestPath(p);
  if (existsSync(path)) return path;
  mkdirSync(join(p.path, '.hub'), { recursive: true });
  writeFileSync(path, TEMPLATE(p.name.replace(/[^\w-]/g, '-').toLowerCase()));
  return path;
}

/** Short manifest text for the agent's system prompt. */
export function manifestPrompt(p: Project): string | null {
  const { manifest } = readManifest(p);
  if (!manifest) return null;
  const compact = Object.fromEntries(Object.entries(manifest).filter(([, v]) => (Array.isArray(v) ? v.length : v && Object.keys(v as object).length)));
  if (!Object.keys(compact).length) return null;
  return `This project has a hub manifest (.hub/hub.yaml). Use it to resolve references like "check prod grafana" or "restart the db":\n${stringify(compact)}`;
}
