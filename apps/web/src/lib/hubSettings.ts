import type { HubSettings } from '@hubness/shared';
import { api } from './api';
import { qk, queryClient, useApi } from './query';

/** Hub settings (projects folder, default permission), a shared cache for the whole app. */
export function useHubSettings(): HubSettings | null {
  return useApi(qk.settings, () => api.settings(), { staleTime: Infinity }).data ?? null;
}

/** Saved settings from the server replace the cached ones everywhere. */
export function publish(s: HubSettings) {
  queryClient.setQueryData(qk.settings, s);
}
