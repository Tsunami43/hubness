import { randomUUID } from 'node:crypto';
import { existsSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { AgentConfig, AskTarget, Project, Session } from '@hubness/shared';
import { bus } from '../bus.ts';
import { AGENTS, DEFAULT_AGENT_KIND, findAgent } from '../agents/registry.ts';
import { config } from '../config.ts';
import { projects, sessions } from '../db/index.ts';
import { sendMessage } from '../runtime.ts';
import { inboxDirFor } from '../inbox.ts';
import { systemPlatform } from './host.ts';
import {
  containerInspectSummary,
  containerLogs,
  packageInfo,
  unitLogs,
  unitStatus,
} from './monitor.ts';

/**
 * “Ask about this”: gather the object's context (logs, status, config) and start a
 * regular chat with it (its own Inbox folder, like a chat from the start page). The
 * server-safety rules travel in the first message, not in a special project.
 */

const HOME_AGENTS_MD = `# home — system project

This folder is the hub's system project. Sessions here are about the server itself:
Docker containers, systemd units, packages and disks.

Rules for agents:
- Diagnose first: explain the cause in plain words, then propose a concrete fix.
- Never run destructive or state-changing commands (restart, stop, rm, edit configs,
  package installs) without asking the user and getting an explicit yes.
- System units and packages need root; show the exact command for the user instead.
`;

/** The home system project (created at server startup). */
export function ensureHomeProject(): Project {
  const existing = projects.system();
  if (existing) return existing;
  const agentsMd = join(config.homeProjectDir, 'AGENTS.md');
  if (!existsSync(agentsMd)) {
    writeFileSync(agentsMd, HOME_AGENTS_MD);
    for (const a of AGENTS) for (const [name, content] of Object.entries(a.contextFiles ?? {})) writeFileSync(join(config.homeProjectDir, name), content);
  }
  return projects.create('home', config.homeProjectDir, 'system');
}

const fence = (label: string, body: string) => `### ${label}\n\`\`\`\n${body.trim() || '(empty)'}\n\`\`\``;

async function buildContext(target: AskTarget): Promise<{ title: string; intro: string; blocks: string[] }> {
  switch (target.kind) {
    case 'container': {
      const [inspect, logs] = await Promise.all([containerInspectSummary(target.id), containerLogs(target.id, 120)]);
      return {
        title: `Container ${target.id}`,
        intro: `Look at the Docker container \`${target.id}\` on this server.`,
        blocks: [fence('docker inspect (summary)', inspect), fence('docker logs --tail 120', logs)],
      };
    }
    case 'unit': {
      const [status, logs] = await Promise.all([unitStatus(target.id, target.scope), unitLogs(target.id, target.scope, 120)]);
      const launchd = systemPlatform().services === 'launchd';
      return {
        title: `Unit ${target.id}`,
        intro: launchd
          ? `Look at the launchd agent \`${target.id}\` on this macOS server.`
          : `Look at the ${target.scope} systemd unit \`${target.id}\` on this server.`,
        blocks: launchd
          ? [fence('launchctl print', status), fence('logs', logs)]
          : [fence('systemctl status', status), fence('journalctl -n 120', logs)],
      };
    }
    case 'package': {
      return {
        title: `Package ${target.id}`,
        intro: `Look at the package \`${target.id}\` on this server (pacman).`,
        blocks: [fence('pacman -Qi + pacman.log', await packageInfo(target.id))],
      };
    }
  }
}

/** Rules for an agent asked about the server (same spirit as the home project's AGENTS.md). */
const ASK_RULES = `You are helping with the server this hub runs on.
- Diagnose first: explain the cause in plain words, then propose a concrete fix.
- Never run destructive or state-changing commands (restart, stop, rm, edit configs, package installs) without asking the user and getting an explicit yes.
- System units and packages need root: show the exact command for the user instead of running it.`;

export async function askAbout(target: AskTarget, opts: { agent?: AgentConfig; question?: string } = {}): Promise<Session> {
  const ctx = await buildContext(target);
  const agent: AgentConfig = {
    kind: opts.agent?.kind && findAgent(opts.agent.kind) ? opts.agent.kind : DEFAULT_AGENT_KIND,
    model: opts.agent?.model,
    permissionMode: opts.agent?.permissionMode ?? 'ask',
  };
  const prompt = [ctx.intro, opts.question?.trim() ? `User question: ${opts.question.trim()}` : '', ASK_RULES, ...ctx.blocks].filter(Boolean).join('\n\n');
  const id = randomUUID();
  // a regular chat: no project or group, its own folder in Inbox
  const s = sessions.create({ id, projectId: null, groupId: null, workspaceId: null, title: ctx.title, agent, cwd: inboxDirFor(id) });
  bus.emitGlobal({ type: 'session', session: s });
  return sendMessage(s.id, prompt);
}
