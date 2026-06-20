/**
 * usePageVisibility Hook
 *
 * Detects whether the browser tab/page is currently visible.
 * Returns false when the user switches to another tab or minimizes the window.
 * Used to pause polling and WebSocket connections when the page is not visible.
 */

import { useEffect, useState } from 'react';

export function usePageVisibility(): boolean {
  const [isVisible, setIsVisible] = useState(!document.hidden);

  useEffect(() => {
    const handleVisibilityChange = () => {
      setIsVisible(!document.hidden);
    };

    document.addEventListener('visibilitychange', handleVisibilityChange);
    return () => {
      document.removeEventListener('visibilitychange', handleVisibilityChange);
    };
  }, []);

  return isVisible;
}
