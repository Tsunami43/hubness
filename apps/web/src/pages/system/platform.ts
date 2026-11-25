import { useEffect, useState } from 'react';
import type { SystemPlatform } from '@hubness/shared';
import { api } from '../../lib/api';

// the platform never changes while the server runs: one request per page load
let platformReq: Promise<SystemPlatform> | null = null;
export function usePlatform(): SystemPlatform | null {
  const [p, setP] = useState<SystemPlatform | null>(null);
  useEffect(() => {
    platformReq ??= api.sysPlatform().catch((e) => ((platformReq = null), Promise.reject(e)));
    let alive = true;
    platformReq.then((v) => alive && setP(v)).catch(() => {});
    return () => {
      alive = false;
    };
  }, []);
  return p;
}
