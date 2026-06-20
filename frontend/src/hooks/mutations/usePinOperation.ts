/**
 * usePinOperation Hook
 * 
 * Mutation for SIM PIN operations.
 * Invalidates SIM status on success.
 */

import { useMutation, useQueryClient } from '@tanstack/react-query';
import { pinOperation } from '@/api';
import { simStatusQueryKey } from '../queries';
import type { PinRequest, PinResult } from '@/types/api';

export function usePinOperation() {
  const queryClient = useQueryClient();

  return useMutation<PinResult, Error, PinRequest>({
    mutationFn: pinOperation,
    onSuccess: () => {
      // SIM state may have changed (unlocked, locked, etc.)
      queryClient.invalidateQueries({ queryKey: simStatusQueryKey });
    },
  });
}
