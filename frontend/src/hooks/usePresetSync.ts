/**
 * Preset Sync Hook
 *
 * Loads view presets from the server on mount and debounce-syncs
 * changes back to the server when presets are modified.
 */

import { useEffect, useRef } from 'react';
import { useUIStore } from '@/stores/uiStore';
import { getProfile, updateViewPresets } from '@/api/profile';

const SYNC_DEBOUNCE_MS = 1500;

export function usePresetSync() {
  const dirty = useUIStore((s) => s._presetsDirty);
  const timerRef = useRef<ReturnType<typeof setTimeout>>();
  const initedRef = useRef(false);

  // Load presets from server on mount (once)
  useEffect(() => {
    if (initedRef.current) return;
    initedRef.current = true;

    getProfile()
      .then((resp) => {
        useUIStore
          .getState()
          .initPresetsFromServer(resp.profile?.view_presets ?? null);
      })
      .catch(() => {
        // Silent fail — use localStorage presets as fallback
      });
  }, []);

  // Debounced sync to server when presets change
  useEffect(() => {
    if (!dirty) return;

    clearTimeout(timerRef.current);
    timerRef.current = setTimeout(async () => {
      const { presets, markPresetsClean } = useUIStore.getState();
      try {
        await updateViewPresets(presets);
        markPresetsClean();
      } catch {
        // Will retry on next change
      }
    }, SYNC_DEBOUNCE_MS);

    return () => clearTimeout(timerRef.current);
  }, [dirty]);
}
