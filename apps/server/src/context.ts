import type { Project, ProjectGroup, RepoInfo } from '@hubness/shared';
import { manifestPrompt } from './manifest.ts';

/**
 * Building context for the agent (the session system prompt).
 *
 * Levels, from general to specific: hub-wide session instructions → group → project → .hub/hub.yaml manifest →
 * session (user messages). A more specific level refines a general one; on
 * conflict the more specific wins — and the agent is told so. Access permissions and
 * security rules are not part of the context and are not overridden by it: the
 * hub enforces them (permission mode, the session's folder set).
 *
 * AGENTS.md / CLAUDE.md files in repositories are read by agents themselves — they are
 * not copied here.
 */

export interface RepoContext {
  project: Project;
  repo: Pick<RepoInfo, 'branch' | 'defaultBranch' | 'providerLabel' | 'remoteUrl'> | null;
}

const section = (title: string, body: string) => (body.trim() ? `## ${title}\n${body.trim()}` : '');

function repoLine({ project, repo }: RepoContext, path: string): string {
  const bits = [
    `path: ${path}`,
    repo?.branch ? `branch: ${repo.branch}` : '',
    repo?.defaultBranch ? `default branch: ${repo.defaultBranch}` : '',
    repo?.remoteUrl ? `remote: ${repo.remoteUrl}${repo.providerLabel ? ` (${repo.providerLabel})` : ''}` : '',
  ].filter(Boolean);
  return `- ${project.name} — ${bits.join(', ')}`;
}

/**
 * Final session system prompt: hub-wide session instructions (Settings → Agent) first, then
 * the scope context (project or group). Scope instructions are more specific and win on
 * conflict. null — nothing to add (e.g. a chat with no hub-wide instructions).
 */
export function sessionSystemPrompt(hubInstructions: string, scope: string | null): string | null {
  const parts = [section('General instructions', hubInstructions), scope ?? ''].filter(Boolean);
  return parts.length ? parts.join('\n\n') : null;
}

/** Project session context: description, instructions, manifest. null — nothing to add. */
export function projectContext(project: Project): string | null {
  const parts = [
    section('Project', project.description),
    section('Project instructions', project.instructions),
    manifestPrompt(project) ?? '',
  ].filter(Boolean);
  return parts.length ? parts.join('\n\n') : null;
}

/**
 * Aggregated group session context: what the product is, which repositories
 * it includes (paths, branches, remotes), and the description and instructions of each.
 */
export function groupContext(group: ProjectGroup, repos: RepoContext[]): string {
  const header = [
    `You are working on "${group.name}" — a group of ${repos.length} repositories that make up one product.`,
    'Your working directory contains a link to each repository; the real paths are listed below and are all available to you.',
    'Only these repositories belong to this session. Instructions closer to a repository override the group-wide ones when they conflict.',
  ].join(' ');
  const perRepo = repos
    .map((r) => {
      const extra = [r.project.description, r.project.instructions ? `Instructions: ${r.project.instructions}` : '', manifestPrompt(r.project) ?? '']
        .filter(Boolean)
        .join('\n');
      return extra ? `### ${r.project.name}\n${extra}` : '';
    })
    .filter(Boolean)
    .join('\n\n');
  return [
    header,
    section('Group', group.description),
    section('Group instructions', group.instructions),
    section('Repositories', repos.map((r) => repoLine(r, r.project.path)).join('\n')),
    perRepo ? section('Repository notes', perRepo) : '',
  ]
    .filter(Boolean)
    .join('\n\n');
}
