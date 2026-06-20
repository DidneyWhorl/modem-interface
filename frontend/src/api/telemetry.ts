/**
 * Telemetry API
 *
 * Manages telemetry opt-in configuration and polling controls.
 */

import { apiGet, apiPost, apiPut } from './client';

export interface TelemetryConfig {
  local_enabled: boolean;
  portal_enabled: boolean;
  active: boolean;
}

export interface PollingState {
  mode: 'normal' | 'fast';
  interval_secs: number;
  fast_mode_remaining_secs: number | null;
  options: number[];
}

export interface PollNowResponse {
  queued: boolean;
}

export function getTelemetryConfig(): Promise<TelemetryConfig> {
  return apiGet<TelemetryConfig>('/telemetry/config');
}

export function updateTelemetryConfig(enabled: boolean): Promise<TelemetryConfig> {
  return apiPut<TelemetryConfig>('/telemetry/config', { enabled });
}

export function getPollingState(): Promise<PollingState> {
  return apiGet<PollingState>('/telemetry/polling');
}

export function updatePollingMode(mode: 'normal' | 'fast', interval_secs?: number): Promise<PollingState> {
  return apiPut<PollingState>('/telemetry/polling', { mode, interval_secs });
}

export function triggerPollNow(): Promise<PollNowResponse> {
  return apiPost<PollNowResponse>('/telemetry/poll-now');
}
