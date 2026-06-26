/**
 * License API
 *
 * Handles license status checks and activation.
 */

import { apiGet, apiPost } from './client';
import type { LicenseStatus, PublicLicenseStatus } from '@/types/api';

/**
 * Public, unauthenticated status — only `state` + `device_token`. Safe to call
 * before login (activation screen, optional cloud-license settings panel).
 */
export function getLicenseStatus(): Promise<PublicLicenseStatus> {
  return apiGet<PublicLicenseStatus>('/license/status');
}

/**
 * Authenticated full detail — includes tier/expiry/user_id for the dashboard
 * profile display. Requires a valid session (call only when authenticated).
 */
export function getLicenseDetail(): Promise<LicenseStatus> {
  return apiGet<LicenseStatus>('/license/detail');
}

export function activateLicense(licenseKey: string): Promise<LicenseStatus> {
  return apiPost<LicenseStatus>('/license/activate', { license_key: licenseKey });
}
