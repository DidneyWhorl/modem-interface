import { useQuery } from '@tanstack/react-query';
import { getBandConfig } from '@/api';
import type { BandConfigResponse } from '@/types/api';

export const bandConfigQueryKey = (modemId: string) => ['modem', modemId, 'bands'] as const;

interface UseBandConfigOptions {
  modemId: string | undefined;
  enabled?: boolean;
}

export function useBandConfig(options: UseBandConfigOptions) {
  const { modemId, enabled = true } = options;

  return useQuery<BandConfigResponse>({
    queryKey: bandConfigQueryKey(modemId!),
    queryFn: () => getBandConfig(modemId!),
    enabled: !!modemId && enabled,
    staleTime: 30_000,
    refetchOnWindowFocus: false,
    retry: 1,
  });
}
