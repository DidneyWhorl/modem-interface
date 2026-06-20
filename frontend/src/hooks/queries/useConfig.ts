/**
 * useConfig Hook
 * 
 * Fetches persistent modem configuration.
 */

import { useQuery } from '@tanstack/react-query';
import { getConfig } from '@/api';
import type { ModemConfig } from '@/types/api';

export const configQueryKey = ['config'] as const;

export function useConfig() {
  return useQuery<ModemConfig>({
    queryKey: configQueryKey,
    queryFn: getConfig,
    // Config rarely changes
    staleTime: 60_000,
    refetchOnWindowFocus: false,
  });
}
