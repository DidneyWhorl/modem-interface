/**
 * WAN Manager API Functions
 *
 * Maps to:
 *   GET  /api/wan/status
 *   PUT  /api/wan/config
 *   POST /api/wan/scan
 */

import { apiGet, apiPost, apiPut } from './client';
import type {
  WanStatusResponse, WanConfig,
  WanWatchdogLogResponse,
  WanScanResponse, AddEthernetRequest,
} from '@/types/api';

export async function getWanStatus(): Promise<WanStatusResponse> {
  return apiGet<WanStatusResponse>('/wan/status');
}

export async function updateWanConfig(config: WanConfig): Promise<WanStatusResponse> {
  return apiPut<WanStatusResponse, WanConfig>('/wan/config', config);
}

export async function scanWanModems(): Promise<WanScanResponse> {
  return apiPost<WanScanResponse>('/wan/scan');
}

export async function addEthernetPort(req: AddEthernetRequest): Promise<WanConfig> {
  return apiPost<WanConfig, AddEthernetRequest>('/wan/add-ethernet', req);
}

export async function failbackNow(): Promise<WanStatusResponse> {
  return apiPost<WanStatusResponse>('/wan/failback');
}

export async function acceptFailover(): Promise<WanStatusResponse> {
  return apiPost<WanStatusResponse>('/wan/accept-failover');
}

export async function getWatchdogLog(): Promise<WanWatchdogLogResponse> {
  return apiGet<WanWatchdogLogResponse>('/wan/watchdog/log');
}

export async function clearRestartSuspensions(): Promise<WanStatusResponse> {
  return apiPost<WanStatusResponse>('/wan/watchdog/restart-suspension/clear');
}

export async function clearWatchdogLog(): Promise<void> {
  await apiPost('/wan/watchdog/log/clear');
}

export async function downloadWatchdogLog(): Promise<void> {
  // Fetch raw text and trigger browser download
  const response = await fetch('/api/wan/watchdog/log/download', { credentials: 'include' });
  const text = await response.text();
  const blob = new Blob([text], { type: 'text/plain' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  const date = new Date().toISOString().slice(0, 10);
  a.href = url;
  a.download = `wan-watchdog-${date}.log`;
  a.click();
  URL.revokeObjectURL(url);
}
