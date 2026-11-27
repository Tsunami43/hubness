import { useCallback } from 'react';
import { QueryClient, useQuery, useQueryClient, type QueryKey } from '@tanstack/react-query';

/**
 * Server data cache for the whole app (TanStack Query): one entry per query key, shared by
 * every screen that reads it, deduplicated in flight, updated in place by live streams.
 */
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // files and git change under us: a revisited screen shows the cache at once and refetches in the background
      staleTime: 0,
      retry: 1,
      refetchOnWindowFocus: false,
    },
  },
});

/** Query keys: the first element names the resource (invalidating it refreshes every variant). */
export const qk = {
  agents: ['agents'] as const,
  settings: ['settings'] as const,
  voice: ['voice'] as const,
  sessions: (scope: 'all' | 'archived' | `p:${string}` | `g:${string}`) => ['sessions', scope] as const,
};

/**
 * A server read: `{ data, setData, error, loading, reload }` over a cached query.
 * `key` must name everything the request depends on.
 */
export function useApi<T>(
  key: QueryKey,
  fn: () => Promise<T>,
  /** `keepWhile`: while a new key loads, keep showing the previous key's data if this says so. */
  opts: { enabled?: boolean; staleTime?: number; keepWhile?: (previousKey: QueryKey) => boolean } = {},
) {
  const client = useQueryClient();
  const keepWhile = opts.keepWhile;
  const q = useQuery<T>({
    queryKey: key,
    queryFn: fn,
    enabled: opts.enabled,
    staleTime: opts.staleTime,
    ...(keepWhile
      ? { placeholderData: ((prev: unknown, prevQuery?: { queryKey: QueryKey }) => (prevQuery && keepWhile(prevQuery.queryKey) ? prev : undefined)) as never }
      : {}),
  });
  const keyStr = JSON.stringify(key);
  const setData = useCallback(
    (v: T | undefined | ((prev: T | undefined) => T | undefined)) => client.setQueryData<T>(JSON.parse(keyStr) as QueryKey, v as never),
    [client, keyStr],
  );
  const reload = useCallback(async () => {
    await q.refetch();
  }, [q.refetch]);
  return {
    data: q.data,
    setData,
    error: q.error ? (q.error as Error).message : null,
    loading: q.isFetching,
    reload,
  };
}
