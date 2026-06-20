/**
 * Authentication Hook
 *
 * Manages auth state: loading → authenticated | unauthenticated | setup_required.
 * Carries user identity (username + role + panel restrictions) when authenticated.
 * Fetches profile after login to get allowed_panels.
 * Listens for 401 responses from the API client to trigger re-authentication.
 */

import { useState, useEffect, useCallback } from 'react';
import {
  getAuthStatus,
  login as apiLogin,
  logout as apiLogout,
  setupPassword as apiSetup,
} from '@/api/auth';
import { getProfile } from '@/api/profile';
import { useUIStore } from '@/stores/uiStore';


export type AuthState = 'loading' | 'authenticated' | 'unauthenticated' | 'setup_required';

export interface UserInfo {
  username: string;
  role: string;
  allowedPanels: string[] | null; // null = unrestricted
  allowedFeatures: string[] | null; // null = unrestricted
}

/** Fetch profile to get allowed_panels and allowed_features. */
async function fetchProfileRestrictions(): Promise<{ allowedPanels: string[] | null; allowedFeatures: string[] | null }> {
  try {
    const profile = await getProfile();
    return { allowedPanels: profile.allowed_panels, allowedFeatures: profile.allowed_features };
  } catch {
    return { allowedPanels: null, allowedFeatures: null }; // If profile fetch fails, default to unrestricted
  }
}

export function useAuth() {
  const [state, setState] = useState<AuthState>('loading');
  const [user, setUser] = useState<UserInfo | null>(null);

  const refresh = useCallback(async () => {
    try {
      const status = await getAuthStatus();
      if (!status.auth_required) {
        setState('authenticated');
        setUser(null); // No auth means no user identity
      } else if (status.setup_required) {
        setState('setup_required');
        setUser(null);
      } else if (status.authenticated) {
        setState('authenticated');
        if (status.username && status.role) {
          const { allowedPanels, allowedFeatures } = await fetchProfileRestrictions();
          setUser({ username: status.username, role: status.role, allowedPanels, allowedFeatures });
        } else {
          setUser(null);
        }
      } else {
        setState('unauthenticated');
        setUser(null);
      }
    } catch {
      setState('unauthenticated');
      setUser(null);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  // Listen for 401 events from API client
  useEffect(() => {
    const handler = () => {
      setState('unauthenticated');
      setUser(null);
    };
    window.addEventListener('auth:unauthorized', handler);
    return () => window.removeEventListener('auth:unauthorized', handler);
  }, []);

  const login = useCallback(async (username: string, password: string): Promise<string | null> => {
    try {
      const result = await apiLogin(username, password);
      if (result.success) {
        setState('authenticated');
        if (result.username && result.role) {
          const { allowedPanels, allowedFeatures } = await fetchProfileRestrictions();
          setUser({ username: result.username, role: result.role, allowedPanels, allowedFeatures });
        } else {
          setUser(null);
        }
        return null; // Success
      }
      return 'Invalid credentials';
    } catch (err: unknown) {
      // Surface backend error message (e.g. "Account is deactivated")
      const message = typeof err === 'object' && err !== null && 'message' in err
        ? String((err as { message: unknown }).message)
        : null;
      return message || 'Invalid credentials';
    }
  }, []);

  const logout = useCallback(async () => {
    try {
      await apiLogout();
    } finally {
      setState('unauthenticated');
      setUser(null);
      useUIStore.getState().clearPresets();
    }
  }, []);

  const setup = useCallback(async (username: string, password: string): Promise<boolean> => {
    try {
      const result = await apiSetup(username, password);
      if (result.success) {
        setState('unauthenticated');
        return true;
      }
      return false;
    } catch {
      return false;
    }
  }, []);

  return { state, user, login, logout, setup, refresh };
}
