import { createContext, useContext, useEffect } from 'react';

/**
 * "Ask the agent" about a selection (code, Run/Shell output): the text goes as a draft
 * into the input field of a session in the scope — the current one or a new one.
 */

/** A session the text can go to. */
export interface AskSession {
  id: string;
  title: string;
}

export interface AskApi {
  /** Put the text into a session's input: a session id or 'new'. */
  ask: (text: string, target: string) => void;
  /**
   * Where "Ask in session" can go: the sessions open in tabs (the one in view first); with
   * none open — the last opened session of the scope.
   */
  sessions: AskSession[];
}

/** Set inside a project/group workspace; outside it — null (no buttons). */
export const AskContext = createContext<AskApi | null>(null);
export const useAsk = () => useContext(AskContext);

// drafts for input fields: keyed by scope (project/group)
const drafts = new Map<string, string>();
const listeners = new Map<string, Set<(text: string) => void>>();

export function pushDraft(key: string, text: string) {
  const ls = listeners.get(key);
  if (ls?.size) for (const fn of ls) fn(text);
  else drafts.set(key, (drafts.get(key) ? drafts.get(key) + '\n\n' : '') + text);
}

/** An input field subscribes to drafts of its scope (and picks up a pending one). */
export function useDraft(key: string | undefined, onDraft: (text: string) => void) {
  useEffect(() => {
    if (!key) return;
    const pending = drafts.get(key);
    if (pending) {
      drafts.delete(key);
      onDraft(pending);
    }
    const set = listeners.get(key) ?? new Set();
    set.add(onDraft);
    listeners.set(key, set);
    return () => void set.delete(onDraft);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
}

/** The last opened session of the scope — the "current" one for "ask in session". */
export const lastSessionKey = (scopeKey: string) => `hub.lastSession.${scopeKey}`;
