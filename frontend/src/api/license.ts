/**
 * License API
 *
 * Handles license status checks and activation.
 */

import { apiGet, apiPost } from './client';
import type { LicenseStatus } from '@/types/api';

export function getLicenseStatus(): Promise<LicenseStatus> {
  return apiGet<LicenseStatus>('/license/status');
}

export function activateLicense(licenseKey: string): Promise<LicenseStatus> {
  return apiPost<LicenseStatus>('/license/activate', { license_key: licenseKey });
}
