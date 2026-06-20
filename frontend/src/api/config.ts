/**
 * Network & Configuration API Functions
 * 
 * Maps to:
 *   GET  /api/network/scan
 *   POST /api/network/select
 *   GET  /api/config
 *   PUT  /api/config
 */

import { apiGet, apiPost, apiPut } from './client';
import type {
  NetworkScanResult,
  AvailableNetwork,
  ModemConfig,
} from '@/types/api';

/**
 * Scan for available cellular networks.
 * Note: This can take 30-60 seconds and temporarily disrupts connection.
 */
export async function scanNetworks(): Promise<NetworkScanResult> {
  return apiGet<NetworkScanResult>('/network/scan');
}

/**
 * Manually select a network operator.
 */
export async function selectNetwork(
  network: Pick<AvailableNetwork, 'mcc' | 'mnc'>
): Promise<{ success: boolean }> {
  return apiPost<{ success: boolean }>('/network/select', network);
}

/**
 * Get current modem configuration.
 */
export async function getConfig(): Promise<ModemConfig> {
  return apiGet<ModemConfig>('/config');
}

/**
 * Update modem configuration.
 */
export async function updateConfig(config: Partial<ModemConfig>): Promise<ModemConfig> {
  return apiPut<ModemConfig>('/config', config);
}
