/**
 * User Management API
 *
 * CRUD operations for user accounts (Admin+ only).
 */

import { apiGet, apiPost, apiPut, apiDelete } from './client';

export interface UserInfo {
  username: string;
  role: string;
  allowed_panels: string[] | null;
  allowed_features: string[] | null;
  disabled: boolean;
}

interface UserListResponse {
  users: UserInfo[];
}

interface SuccessResponse {
  success: boolean;
}

export interface CreateUserRequest {
  username: string;
  password: string;
  role?: string;
  allowed_panels?: string[] | null;
  allowed_features?: string[] | null;
}

export interface UpdateUserRequest {
  role?: string;
  allowed_panels?: string[] | null;
  allowed_features?: string[] | null;
  disabled?: boolean;
}

export async function listUsers(): Promise<UserInfo[]> {
  const res = await apiGet<UserListResponse>('/users');
  return res.users;
}

export function createUser(req: CreateUserRequest): Promise<SuccessResponse> {
  return apiPost<SuccessResponse>('/users', req);
}

export function updateUser(username: string, req: UpdateUserRequest): Promise<SuccessResponse> {
  return apiPut<SuccessResponse, UpdateUserRequest>(`/users/${encodeURIComponent(username)}`, req);
}

export function deleteUser(username: string): Promise<SuccessResponse> {
  return apiDelete<SuccessResponse>(`/users/${encodeURIComponent(username)}`);
}

export function resetUserPassword(username: string, newPassword: string): Promise<SuccessResponse> {
  return apiPost<SuccessResponse>(`/users/${encodeURIComponent(username)}/reset-password`, {
    new_password: newPassword,
  });
}
