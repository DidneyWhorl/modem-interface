/**
 * Theme Hook
 *
 * Reads the active theme from the UI store and applies it
 * to the document root element via data-theme attribute.
 * Handles 'system' mode by detecting OS/browser preference.
 * Call once in App.tsx.
 */

import { useEffect } from 'react';
import { useUIStore } from '@/stores/uiStore';

type ResolvedTheme = 'light' | 'dark' | 'fallen';

function resolveSystemTheme(): 'light' | 'dark' {
  if (typeof window === 'undefined') return 'dark';
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

export function useTheme() {
  const theme = useUIStore((s) => s.theme);

  useEffect(() => {
    const root = document.documentElement;

    const apply = (resolved: ResolvedTheme) => {
      root.setAttribute('data-theme', resolved);
      if (resolved === 'light') {
        root.classList.remove('dark');
      } else {
        root.classList.add('dark');
      }
    };

    if (theme === 'system') {
      // Apply based on current OS preference
      apply(resolveSystemTheme());

      // Listen for OS preference changes
      const mql = window.matchMedia('(prefers-color-scheme: dark)');
      const handler = (e: MediaQueryListEvent) => {
        apply(e.matches ? 'dark' : 'light');
      };
      mql.addEventListener('change', handler);
      return () => mql.removeEventListener('change', handler);
    } else {
      apply(theme);
    }
  }, [theme]);

  return theme;
}
