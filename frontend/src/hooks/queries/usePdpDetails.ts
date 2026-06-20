/**
 * usePdpDetails Hook
 *
 * Fetches PDP context details and MBN carrier profile configuration.
 * Shared query key ['modem', 'pdp'] — invalidated by apply/reconnect mutations
 * and by the MBN config mutations already in the app.
 *
 * The query is enabled and runs on mount. staleTime: Infinity means React Query
 * will not automatically refetch stale data in the background; callers that need
 * a guaranteed fresh read (e.g. ApnEditor on mount and after a successful apply)
 * call refetch() explicitly to bypass the stale window.
 */

import { useQuery } from '@tanstack/react-query';
import { getPdpDetails } from '@/api/modem';
import type { PdpDetails } from '@/api/modem';

export const pdpDetailsQueryKey = ['modem', 'pdp'] as const;

export function usePdpDetails() {
  return useQuery<PdpDetails>({
    queryKey: pdpDetailsQueryKey,
    queryFn: getPdpDetails,
    staleTime: Infinity,
    refetchOnWindowFocus: false,
    retry: 1,
  });
}
