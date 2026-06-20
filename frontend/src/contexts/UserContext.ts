/**
 * User Context
 *
 * Provides the currently authenticated user info to any child component.
 * Set at the Dashboard level in App.tsx.
 */

import { createContext, useContext } from 'react';
import type { UserInfo } from '@/hooks/useAuth';

export const UserContext = createContext<UserInfo | null>(null);
export const useCurrentUser = () => useContext(UserContext);
