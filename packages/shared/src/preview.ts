/** A console / error / failed-request entry reported from a previewed app. */
export interface PreviewLog {
  seq: number;
  ts: string;
  level: 'error' | 'warn' | 'log' | 'network';
  text: string;
  /** The page it came from. */
  url: string;
  /** `user` — the Preview panel or a browser tab; `agent` — the hub's headless browser. */
  source: 'user' | 'agent';
}

/** `GET /api/projects/:id/previews/:port/logs?since=N`. */
export interface PreviewLogsResponse {
  logs: PreviewLog[];
}

/** What the agent's headless browser can do (`POST /api/sessions/:id/browser`). */
export type BrowserAction = 'open' | 'screenshot' | 'click' | 'type' | 'press' | 'eval' | 'console' | 'close';

export interface BrowserRequest {
  action: BrowserAction;
  /** open: the address (http://localhost:5173/… is routed through the project's preview). */
  url?: string;
  /** open: the window size (default 1280×800; under 600 wide counts as a phone). */
  width?: number;
  height?: number;
  /** click / type: a CSS selector… */
  selector?: string;
  /** …or the visible text of the element (click). */
  text?: string;
  /** type: what to type (replaces the field's value). */
  value?: string;
  /** press: a key — Enter, Tab, Escape, Backspace, ArrowDown… */
  key?: string;
  /** eval: a JavaScript expression (awaited; the result is returned as JSON). */
  expression?: string;
  /** screenshot: the whole page, not only the window. */
  fullPage?: boolean;
  /** open / click / type / press: attach a screenshot after the action. */
  screenshot?: boolean;
}

export interface BrowserResult {
  ok: boolean;
  /** The page now (as the agent addressed it). */
  url: string;
  title: string;
  /** A message: an error, the eval result… */
  text: string;
  /** PNG, base64. */
  screenshot?: string;
  /** Console / errors / failed requests since the previous call (agent's page), then the user's. */
  logs: PreviewLog[];
}
