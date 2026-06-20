/**
 * System API Functions
 *
 * Maps to:
 *   GET  /api/system/version
 *   GET  /api/system/update/check
 *   POST /api/system/update/apply
 *   GET  /api/system/update/status
 *   GET  /api/system/update/log
 */

import { apiGet, apiPost } from './client';

export interface VersionInfo {
  current_version: string;
}

export interface UpdateCheckResult {
  update_available: boolean;
  installed_version: string;
  available_version: string | null;
  debug_log?: string[];
}

export interface UpdateApplyResult {
  accepted: boolean;
  message: string;
}

export interface UpdateStatus {
  status: 'idle' | 'checking' | 'updating' | 'completed' | 'failed';
  previous_version?: string;
  new_version?: string;
  timestamp?: string;
}

export function getVersion(): Promise<VersionInfo> {
  return apiGet<VersionInfo>('/system/version');
}

export function checkForUpdate(): Promise<UpdateCheckResult> {
  return apiGet<UpdateCheckResult>('/system/update/check');
}

export function applyUpdate(): Promise<UpdateApplyResult> {
  return apiPost<UpdateApplyResult>('/system/update/apply');
}

export function getUpdateStatus(): Promise<UpdateStatus> {
  return apiGet<UpdateStatus>('/system/update/status');
}

export function getUpdateLog(): Promise<string[]> {
  return apiGet<string[]>('/system/update/log');
}
