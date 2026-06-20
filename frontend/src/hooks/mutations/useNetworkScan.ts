/**
 * useNetworkScan / useNetworkSelect Hooks
 * 
 * Mutations for network scanning and manual selection.
 */

import { useMutation, useQueryClient } from '@tanstack/react-query';
import { scanNetworks, selectNetwork } from '@/api';
import { modemStatusQueryKey } from '../queries';
import type { NetworkScanResult, AvailableNetwork } from '@/types/api';

export function useNetworkScan() {
  return useMutation<NetworkScanResult, Error, void>({
    mutationFn: scanNetworks,
    // Network scan can take 30-60 seconds
    // Results aren't cached as networks change frequently
  });
}

export function useNetworkSelect() {
  const queryClient = useQueryClient();

  return useMutation<{ success: boolean }, Error, Pick<AvailableNetwork, 'mcc' | 'mnc'>>({
    mutationFn: selectNetwork,
    onSuccess: () => {
      // Network selection affects registration status
      queryClient.invalidateQueries({ queryKey: modemStatusQueryKey });
    },
  });
}
