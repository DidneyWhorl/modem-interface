/**
 * useApplyApn / useReconnect Hooks
 *
 * Mutations for the diff-aware APN apply and radio-cycle reconnect flows
 * introduced in Item #42 Phase 3.
 *
 * On success both hooks invalidate:
 *   - ['modem', 'pdp']   — PDP context / current_config
 *   - ['modem', 'status'] — connection state / IP address
 */

import { useMutation, useQueryClient } from '@tanstack/react-query';
import { applyApn, reconnect } from '@/api';
import { pdpDetailsQueryKey } from '../queries/usePdpDetails';
import { modemStatusQueryKey } from '../queries/useModemStatus';
import type { ApnApplyRequest, ApnApplyResult, ModemStatus } from '@/types/api';

export function useApplyApn() {
  const queryClient = useQueryClient();

  return useMutation<ApnApplyResult, Error, { modemId: string; req: ApnApplyRequest }>({
    mutationFn: ({ modemId, req }) => applyApn(modemId, req),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: pdpDetailsQueryKey });
      queryClient.invalidateQueries({ queryKey: modemStatusQueryKey });
    },
  });
}

export function useReconnect() {
  const queryClient = useQueryClient();

  return useMutation<ModemStatus, Error, { modemId: string }>({
    mutationFn: ({ modemId }) => reconnect(modemId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: pdpDetailsQueryKey });
      queryClient.invalidateQueries({ queryKey: modemStatusQueryKey });
    },
  });
}
