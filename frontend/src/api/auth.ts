/**
 * Authentication API
 *
 * Handles multi-user login, logout, status check, first-run setup,
 * and password change.
 */

import { apiGet, apiPost } from './client';

export interface AuthStatus {
  authenticated: boolean;
  auth_required: boolean;
  setup_required: boolean;
  username?: string;
  role?: string;
}

export interface LoginResult {
  success: boolean;
  username?: string;
  role?: string;
}

export interface AuthResult {
  success: boolean;
}

export function getAuthStatus(): Promise<AuthStatus> {
  return apiGet<AuthStatus>('/auth/status');
}

export function login(username: string, password: string): Promise<LoginResult> {
  return apiPost<LoginResult>('/auth/login', { username, password });
}

export function logout(): Promise<AuthResult> {
  return apiPost<AuthResult>('/auth/logout');
}

export function setupPassword(username: string, password: string): Promise<AuthResult> {
  return apiPost<AuthResult>('/auth/setup', { username, password });
}

export function changePassword(currentPassword: string, newPassword: string): Promise<AuthResult> {
  return apiPost<AuthResult>('/auth/change-password', {
    current_password: currentPassword,
    new_password: newPassword,
  });
}

/** Fetch a single-use WebSocket auth token (30s TTL). */
export function fetchWsToken(): Promise<{ token: string }> {
  return apiPost<{ token: string }>('/auth/ws-token');
}
