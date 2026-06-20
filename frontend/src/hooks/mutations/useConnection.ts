/**
 * useConnect / useDisconnect Hooks
 * 
 * Mutations for establishing and terminating data connections.
 * Automatically invalidates status queries on success.
 */

import { useMutation, useQueryClient } from '@tanstack/react-query';
import { connect, disconnect } from '@/api';
import { modemStatusQueryKey, signalQueryKey, configQueryKey } from '../queries';
import type { ConnectionConfig, ConnectionResult } from '@/types/api';

export function useConnect() {
  const queryClient = useQueryClient();

  return useMutation<ConnectionResult, Error, ConnectionConfig>({
    mutationFn: connect,
    onSuccess: () => {
      // Invalidate status queries to reflect new connection state
      queryClient.invalidateQueries({ queryKey: modemStatusQueryKey });
      queryClient.invalidateQueries({ queryKey: signalQueryKey });
      // Refresh config so connection form shows saved APN
      queryClient.invalidateQueries({ queryKey: configQueryKey });
    },
  });
}

export function useDisconnect() {
  const queryClient = useQueryClient();

  return useMutation<{ success: boolean }, Error, void>({
    mutationFn: disconnect,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: modemStatusQueryKey });
    },
  });
}
