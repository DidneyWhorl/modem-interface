/**
 * Profile API
 *
 * Fetches and updates the authenticated user's profile including
 * panel restrictions and view presets.
 */

import { apiGet, apiPut } from './client';
import type { ViewPreset } from '@/types/presets';

export interface ServerUiProfile {
  theme: string;
  sidebar_collapsed: boolean;
  layouts?: unknown;
  visible_panels?: string[];
  view_presets?: ViewPreset[];
}

export interface ProfileResponse {
  username: string;
  role: string;
  allowed_panels: string[] | null;
  allowed_features: string[] | null;
  profile: ServerUiProfile;
}

export async function getProfile(): Promise<ProfileResponse> {
  return apiGet<ProfileResponse>('/profile');
}

export async function updateViewPresets(presets: ViewPreset[]): Promise<void> {
  await apiPut('/profile', { view_presets: presets });
}
