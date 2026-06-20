import { useMutation, useQueryClient } from '@tanstack/react-query';
import {
  createSteeringRule,
  updateSteeringRule,
  deleteSteeringRule,
  reorderSteeringRules,
} from '@/api/steering';
import type {
  SteeringRule,
  SteeringListResponse,
  CreateSteeringRuleRequest,
  UpdateSteeringRuleRequest,
  ReorderSteeringRequest,
} from '@/types/steering';
import { steeringQueryKey } from '../queries/useSteeringRules';

export function useCreateSteeringRule() {
  const queryClient = useQueryClient();
  return useMutation<SteeringRule, Error, CreateSteeringRuleRequest>({
    mutationFn: createSteeringRule,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: steeringQueryKey });
    },
  });
}

export function useUpdateSteeringRule() {
  const queryClient = useQueryClient();
  return useMutation<
    SteeringRule,
    Error,
    { id: string; req: UpdateSteeringRuleRequest }
  >({
    mutationFn: ({ id, req }) => updateSteeringRule(id, req),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: steeringQueryKey });
    },
  });
}

export function useDeleteSteeringRule() {
  const queryClient = useQueryClient();
  return useMutation<void, Error, string>({
    mutationFn: deleteSteeringRule,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: steeringQueryKey });
    },
  });
}

export function useReorderSteeringRules() {
  const queryClient = useQueryClient();
  return useMutation<SteeringListResponse, Error, ReorderSteeringRequest>({
    mutationFn: reorderSteeringRules,
    onSuccess: (data) => {
      queryClient.setQueryData(steeringQueryKey, data);
    },
  });
}
