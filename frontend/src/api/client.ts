/**
 * Base API Client
 * 
 * Provides typed fetch wrapper with consistent error handling.
 * In production, requests go to the same origin (embedded in Rust binary).
 * In development, Vite proxies /api/* to the Rust dev server.
 */

import type { ApiError } from '@/types/api';

const API_BASE = '/ctrl-modem/api';

export class ApiClientError extends Error {
  constructor(
    public code: string,
    message: string,
    public status: number,
    public details?: Record<string, unknown>
  ) {
    super(message);
    this.name = 'ApiClientError';
  }
}

async function handleResponse<T>(response: Response): Promise<T> {
  if (!response.ok) {
    // Signal auth state change on 401 (except for auth endpoints themselves)
    if (response.status === 401 && !response.url.includes('/auth/')) {
      window.dispatchEvent(new CustomEvent('auth:unauthorized'));
    }

    let error: ApiError;

    try {
      error = await response.json();
    } catch {
      error = {
        code: 'UNKNOWN_ERROR',
        message: response.statusText || 'An unknown error occurred',
      };
    }

    throw new ApiClientError(
      error.code,
      error.message,
      response.status,
      error.details
    );
  }
  
  // Handle 204 No Content
  if (response.status === 204) {
    return undefined as T;
  }
  
  return response.json();
}

export async function apiGet<T>(endpoint: string): Promise<T> {
  const response = await fetch(`${API_BASE}${endpoint}`, {
    method: 'GET',
    headers: {
      'Accept': 'application/json',
    },
    credentials: 'same-origin',
  });
  
  return handleResponse<T>(response);
}

export async function apiPost<T, B = unknown>(
  endpoint: string,
  body?: B
): Promise<T> {
  const response = await fetch(`${API_BASE}${endpoint}`, {
    method: 'POST',
    headers: {
      'Accept': 'application/json',
      'Content-Type': 'application/json',
    },
    credentials: 'same-origin',
    body: body ? JSON.stringify(body) : undefined,
  });
  
  return handleResponse<T>(response);
}

export async function apiPut<T, B = unknown>(
  endpoint: string,
  body: B
): Promise<T> {
  const response = await fetch(`${API_BASE}${endpoint}`, {
    method: 'PUT',
    headers: {
      'Accept': 'application/json',
      'Content-Type': 'application/json',
    },
    credentials: 'same-origin',
    body: JSON.stringify(body),
  });
  
  return handleResponse<T>(response);
}

export async function apiDelete<T>(endpoint: string): Promise<T> {
  const response = await fetch(`${API_BASE}${endpoint}`, {
    method: 'DELETE',
    headers: {
      'Accept': 'application/json',
    },
    credentials: 'same-origin',
  });

  return handleResponse<T>(response);
}

/**
 * Create a WebSocket connection to the events endpoint.
 * Returns the WebSocket instance for manual management.
 */
export function createEventSocket(): WebSocket {
  const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const host = window.location.host;
  return new WebSocket(`${protocol}//${host}${API_BASE}/events`);
}
