/**
 * Traffic Steering API Functions
 *
 * Maps to:
 *   GET    /api/wan/steering
 *   POST   /api/wan/steering
 *   PUT    /api/wan/steering/:id
 *   DELETE /api/wan/steering/:id
 *   PUT    /api/wan/steering/reorder
 */

import { apiGet, apiPost, apiPut, apiDelete } from './client';
import type {
  SteeringListResponse,
  SteeringRule,
  CreateSteeringRuleRequest,
  UpdateSteeringRuleRequest,
  ReorderSteeringRequest,
} from '@/types/steering';

export async function getSteeringRules(): Promise<SteeringListResponse> {
  return apiGet<SteeringListResponse>('/wan/steering');
}

export async function createSteeringRule(
  req: CreateSteeringRuleRequest
): Promise<SteeringRule> {
  return apiPost<SteeringRule, CreateSteeringRuleRequest>('/wan/steering', req);
}

export async function updateSteeringRule(
  id: string,
  req: UpdateSteeringRuleRequest
): Promise<SteeringRule> {
  return apiPut<SteeringRule, UpdateSteeringRuleRequest>(
    `/wan/steering/${id}`,
    req
  );
}

export async function deleteSteeringRule(id: string): Promise<void> {
  return apiDelete<void>(`/wan/steering/${id}`);
}

export async function reorderSteeringRules(
  req: ReorderSteeringRequest
): Promise<SteeringListResponse> {
  return apiPut<SteeringListResponse, ReorderSteeringRequest>(
    '/wan/steering/reorder',
    req
  );
}
