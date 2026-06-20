import { useQuery } from '@tanstack/react-query';
import { getSimSlots } from '@/api';
import type { DualSimInfo } from '@/types/api';

export const simSlotsQueryKey = ['sim', 'slots'] as const;

export function useSimSlots() {
  return useQuery<DualSimInfo>({
    queryKey: simSlotsQueryKey,
    queryFn: getSimSlots,
    staleTime: 30_000,
    refetchOnWindowFocus: false,
    retry: 1,
  });
}
