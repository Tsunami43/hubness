/**
 * Server-sent events that survive a hub restart. The browser reconnects an EventSource by
 * itself only after a network drop; when the dev proxy answers with an HTTP error while the
 * server restarts, the EventSource closes for good — then we reconnect with a backoff.
 * `onOpen` fires on every (re)connect: streams that don't send a snapshot reload there.
 */
export function liveStream(url: string, onMessage: (data: string) => void, opts: { onOpen?: () => void; onDown?: () => void } = {}): () => void {
  let es: EventSource | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let delay = 1000;
  let closed = false;
  const connect = () => {
    es = new EventSource(url);
    es.onopen = () => {
      delay = 1000;
      opts.onOpen?.();
    };
    es.onmessage = (m) => onMessage(m.data as string);
    es.onerror = () => {
      opts.onDown?.();
      if (closed || es?.readyState !== EventSource.CLOSED) return;
      timer = setTimeout(connect, delay);
      delay = Math.min(delay * 2, 10_000);
    };
  };
  connect();
  return () => {
    closed = true;
    clearTimeout(timer);
    es?.close();
  };
}
