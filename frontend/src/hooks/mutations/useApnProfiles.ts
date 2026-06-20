import { useMutation, useQueryClient } from '@tanstack/react-query';
import { createApnProfile, updateApnProfile, deleteApnProfile, applyApnProfile, importApnProfiles } from '@/api';
import type { ApnProfile, ApnProfileRequest, ApnProfileApplyRequest, ApnProfileApplyResult, ApnProfileImportResult } from '@/types/api';
import { apnProfilesQueryKey } from '../queries/useApnProfiles';
import { configQueryKey } from '../queries/useConfig';
import { modemStatusQueryKey } from '../queries/useModemStatus';

export function useCreateApnProfile() {
  const queryClient = useQueryClient();
  return useMutation<ApnProfile, Error, { modemId: string; req: ApnProfileRequest }>({
    mutationFn: ({ modemId, req }) => createApnProfile(modemId, req),
    onSuccess: (_data, { modemId }) => {
      queryClient.invalidateQueries({ queryKey: apnProfilesQueryKey(modemId) });
    },
  });
}

export function useUpdateApnProfile() {
  const queryClient = useQueryClient();
  return useMutation<ApnProfile, Error, { modemId: string; id: string; req: ApnProfileRequest }>({
    mutationFn: ({ modemId, id, req }) => updateApnProfile(modemId, id, req),
    onSuccess: (_data, { modemId }) => {
      queryClient.invalidateQueries({ queryKey: apnProfilesQueryKey(modemId) });
    },
  });
}

export function useDeleteApnProfile() {
  const queryClient = useQueryClient();
  return useMutation<{ success: boolean }, Error, { modemId: string; id: string }>({
    mutationFn: ({ modemId, id }) => deleteApnProfile(modemId, id),
    onSuccess: (_data, { modemId }) => {
      queryClient.invalidateQueries({ queryKey: apnProfilesQueryKey(modemId) });
    },
  });
}

export function useApplyApnProfile() {
  const queryClient = useQueryClient();
  return useMutation<ApnProfileApplyResult, Error, { modemId: string; req: ApnProfileApplyRequest }>({
    mutationFn: ({ modemId, req }) => applyApnProfile(modemId, req),
    onSuccess: (_data, { modemId }) => {
      queryClient.invalidateQueries({ queryKey: apnProfilesQueryKey(modemId) });
      queryClient.invalidateQueries({ queryKey: configQueryKey });
      queryClient.invalidateQueries({ queryKey: modemStatusQueryKey });
    },
  });
}

export function useImportApnProfiles() {
  const queryClient = useQueryClient();
  return useMutation<ApnProfileImportResult, Error, { modemId: string; profiles: ApnProfileRequest[] }>({
    mutationFn: ({ modemId, profiles }) => importApnProfiles(modemId, profiles),
    onSuccess: (_data, { modemId }) => {
      queryClient.invalidateQueries({ queryKey: apnProfilesQueryKey(modemId) });
    },
  });
}
