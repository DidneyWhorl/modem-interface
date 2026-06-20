/**
 * Speedtest API Functions
 *
 * Maps to:
 *   POST /api/speedtest/run
 *   GET  /api/speedtest/status
 *   GET  /api/speedtest/history
 */

import { apiGet, apiPost } from './client';
import type {
  SpeedtestMode,
  RunSpeedtestResponse,
  SpeedtestStatusResponse,
  SpeedtestHistoryResponse,
} from '@/types/api';

export async function runSpeedtest(
  mode: SpeedtestMode,
  wanId: string,
): Promise<RunSpeedtestResponse> {
  return apiPost<RunSpeedtestResponse>('/speedtest/run', { mode, wan_id: wanId });
}

export async function getSpeedtestStatus(): Promise<SpeedtestStatusResponse> {
  return apiGet<SpeedtestStatusResponse>('/speedtest/status');
}

export async function getSpeedtestHistory(
  limit?: number,
  wanId?: string,
): Promise<SpeedtestHistoryResponse> {
  const params = new URLSearchParams();
  if (limit != null) params.set('limit', String(limit));
  if (wanId) params.set('wan_id', wanId);
  const qs = params.toString();
  return apiGet<SpeedtestHistoryResponse>(`/speedtest/history${qs ? `?${qs}` : ''}`);
}
