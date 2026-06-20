/**
 * useModemDetection Hook
 * 
 * Detects connected modems and their protocols.
 * Useful for initial setup and troubleshooting.
 */

import { useQuery } from '@tanstack/react-query';
import { detectModems } from '@/api';
import type { DetectedModem } from '@/types/api';

export const modemDetectionQueryKey = ['modem', 'detect'] as const;

export function useModemDetection(options?: { enabled?: boolean }) {
  return useQuery<DetectedModem[]>({
    queryKey: modemDetectionQueryKey,
    queryFn: detectModems,
    // Detection is expensive, don't auto-refetch
    staleTime: Infinity,
    refetchOnWindowFocus: false,
    enabled: options?.enabled ?? true,
  });
}
