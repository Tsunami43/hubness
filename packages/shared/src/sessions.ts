// Sessions, the event feed, approvals, live streams, kanban cards.

import type { AgentConfig } from './agents.ts';

export type SessionStatus = 'idle' | 'running' | 'waiting' | 'error';

export interface Session {
  id: string;
  /**
   * A session has exactly one scope: a project (projectId), a group (groupId) or
   * a plain chat (both null, an Inbox folder as cwd).
   */
  projectId: string | null;
  groupId: string | null;
  workspaceId: string | null;
  cardId: string | null;
  title: string;
  agent: AgentConfig;
  agentSessionId: string | null;
  /** Working folder of a plain chat (Inbox). null for project sessions. */
  cwd: string | null;
  status: SessionStatus;
  /** Moved to the archive (hidden from the session lists); null — active. */
  archivedAt: string | null;
  createdAt: string;
  updatedAt: string;
}

/**
 * A file attached to a message (pasted, dropped or picked). Stored next to the session's
 * working folder (`.hub/attachments/`), so the agent can open it with its own tools; images
 * also go to the model directly when the agent supports them.
 */
export interface Attachment {
  /** File name in the attachments folder (unique). */
  id: string;
  /** Original file name. */
  name: string;
  mime: string;
  size: number;
  /** Absolute path on the server (what the agent is told). */
  path: string;
}

export type ToolStatus = 'running' | 'ok' | 'error' | 'denied';
export type ApprovalStatus = 'pending' | 'allowed' | 'denied' | 'expired';

export interface AskQuestion {
  question: string;
  header?: string;
  multiSelect?: boolean;
  options: { label: string; description?: string }[];
}

interface EventBase {
  /** Stable event key within the session (for upsert). */
  uid: string;
  seq: number;
  ts: string;
}

export type HubEvent = EventBase &
  (
    | { kind: 'user'; text: string; attachments?: Attachment[] }
    | { kind: 'text'; text: string; streaming?: boolean }
    | { kind: 'thinking'; text: string }
    | {
        kind: 'tool';
        toolId: string;
        name: string;
        input: unknown;
        status: ToolStatus;
        output?: string;
      }
    | {
        kind: 'approval';
        requestId: string;
        toolName: string;
        input: unknown;
        status: ApprovalStatus;
        /** For AskUserQuestion — questions with options. */
        questions?: AskQuestion[];
        answers?: Record<string, string>;
        reason?: string;
      }
    | { kind: 'notice'; level: 'info' | 'error'; text: string }
    | {
        kind: 'result';
        ok: boolean;
        /** The turn was interrupted (Stop or hub restart) rather than failing with an error. */
        stopped?: boolean;
        durationMs?: number;
        /**
         * How the turn was paid for: subscription — by subscription (no money is charged,
         * costUsd is just an estimate at API prices and isn't shown); api — by API key.
         */
        billing?: 'subscription' | 'api';
        costUsd?: number;
        /** Subscription limit usage, if the agent reported it. */
        limit?: { window: string; utilization: number };
        model?: string;
      }
  );

export type HubEventKind = HubEvent['kind'];

/** Session SSE stream messages. */
/** What the agent showed next to the chat (the open_pane MCP tool). */
export interface Pane {
  kind: 'url' | 'file' | 'diff';
  /** URL or file path (absolute or relative to the session folder). */
  target: string;
  title?: string;
  at: string;
}

export type SessionStreamMsg =
  | { type: 'snapshot'; session: Session; events: HubEvent[]; pane?: Pane | null }
  | { type: 'pane'; pane: Pane | null }
  | { type: 'event'; event: HubEvent }
  | { type: 'session'; session: Session };

/** Global SSE stream messages (for lists). */
export type GlobalStreamMsg = { type: 'session'; session: Session } | { type: 'session-deleted'; id: string };

export interface ApprovalDecision {
  decision: 'allow' | 'deny';
  reason?: string;
  answers?: Record<string, string>;
}

export type CardColumn = 'todo' | 'doing' | 'review' | 'done';

export interface Card {
  id: string;
  projectId: string;
  title: string;
  description: string;
  column: CardColumn;
  position: number;
  sessionId: string | null;
  createdAt: string;
  updatedAt: string;
}
