/**
 * UpdatePanel Component
 *
 * Shows current version, checks for updates, and allows
 * manual update application. Handles service restart gracefully
 * with a reconnection polling loop.
 */

import { useState, useEffect, useCallback, useRef } from 'react';
import { Download, RefreshCw, Check, AlertTriangle, Loader2, ArrowRight, ChevronDown, ChevronRight } from 'lucide-react';
import { checkForUpdate } from '@/api/system';
import { useApplyUpdate } from '@/hooks/mutations/useApplyUpdate';
import type { UpdateCheckResult, VersionInfo } from '@/api/system';

type UpdatePhase =
  | 'idle'
  | 'checking'
  | 'available'
  | 'up-to-date'
  | 'applying'
  | 'reconnecting'
  | 'updated'
  | 'error';

export function UpdatePanel() {
  const [phase, setPhase] = useState<UpdatePhase>('idle');
  const [updateInfo, setUpdateInfo] = useState<UpdateCheckResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [reconnectAttempts, setReconnectAttempts] = useState(0);
  const [debugLog, setDebugLog] = useState<string[]>([]);
  const [showDebug, setShowDebug] = useState(false);
  const applyMutation = useApplyUpdate();
  const reconnectTimerRef = useRef<number>();
  const previousVersionRef = useRef<string>('');

  // Store current version for reconnection comparison
  useEffect(() => {
    previousVersionRef.current = __APP_VERSION__;
  }, []);

  const handleCheck = useCallback(async () => {
    setPhase('checking');
    setError(null);
    setDebugLog(['Starting update check...']);
    try {
      const result = await checkForUpdate();
      setUpdateInfo(result);
      if (result.debug_log) {
        setDebugLog(result.debug_log);
      }
      setPhase(result.update_available ? 'available' : 'up-to-date');
    } catch (err) {
      setDebugLog(prev => [...prev, `Error: ${err instanceof Error ? err.message : 'Check failed'}`]);
      setError(err instanceof Error ? err.message : 'Check failed');
      setPhase('error');
    }
  }, []);

  const startReconnectLoop = useCallback(() => {
    let attempts = 0;
    const maxAttempts = 60; // 60 * 2s = 2 minutes

    const poll = () => {
      attempts++;
      setReconnectAttempts(attempts);
      setDebugLog(prev => [...prev, `Poll ${attempts}/${maxAttempts}...`]);

      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), 8000);

      fetch('/ctrl-modem/api/system/version', {
        cache: 'no-store',
        credentials: 'same-origin',
        signal: controller.signal,
      })
        .then(res => {
          if (res.ok) {
            return res.json().then((data: VersionInfo) => {
              if (data.current_version !== previousVersionRef.current) {
                setDebugLog(prev => [...prev, `New version detected: ${data.current_version}`]);
                setPhase('updated');
                setTimeout(() => window.location.reload(), 2000);
              } else if (attempts < maxAttempts) {
                setDebugLog(prev => [...prev, `Same version (${data.current_version}), retrying...`]);
                reconnectTimerRef.current = window.setTimeout(poll, 2000);
              } else {
                setError('Timeout waiting for update');
                setPhase('error');
              }
            });
          } else {
            // Backend is alive but session expired (401/403) — reload to hit login
            setDebugLog(prev => [...prev, `Backend alive (status ${res.status}), reloading...`]);
            setPhase('updated');
            setTimeout(() => window.location.reload(), 2000);
          }
        })
        .catch((err) => {
          const msg = err instanceof Error && err.name === 'AbortError'
            ? 'Poll timed out, retrying...'
            : `Poll failed: ${err instanceof Error ? err.message : 'unknown'}, retrying...`;
          setDebugLog(prev => [...prev, msg]);
          if (attempts < maxAttempts) {
            reconnectTimerRef.current = window.setTimeout(poll, 2000);
          } else {
            setError('Backend did not come back after update');
            setPhase('error');
          }
        })
        .finally(() => clearTimeout(timeout));
    };

    poll();
  }, []);

  const handleApply = useCallback(async () => {
    setPhase('applying');
    setError(null);
    setDebugLog(prev => [...prev, 'Sending apply request...']);
    try {
      await applyMutation.mutateAsync();
      setDebugLog(prev => [...prev, 'Apply accepted, service will restart in ~2s']);
      // Backend accepted — it will die in ~2 seconds
      setTimeout(() => {
        setPhase('reconnecting');
        setReconnectAttempts(0);
        setDebugLog(prev => [...prev, 'Waiting for service restart...']);
        startReconnectLoop();
      }, 3000);
    } catch (err) {
      setDebugLog(prev => [...prev, `Apply error: ${err instanceof Error ? err.message : 'Apply failed'}`]);
      setError(err instanceof Error ? err.message : 'Apply failed');
      setPhase('error');
    }
  }, [applyMutation, startReconnectLoop]);

  useEffect(() => {
    return () => {
      if (reconnectTimerRef.current) {
        clearTimeout(reconnectTimerRef.current);
      }
    };
  }, []);

  return (
    <div className="p-4 space-y-4">
      {/* Current Version */}
      <div className="flex items-center justify-between">
        <span className="text-sm text-theme-text-muted">Installed</span>
        <span className="text-sm font-mono text-theme-text-primary">
          v{__APP_VERSION__}
        </span>
      </div>

      {/* Phase-specific content */}
      {phase === 'idle' && (
        <button
          onClick={handleCheck}
          className="btn-secondary w-full flex items-center justify-center gap-2"
        >
          <RefreshCw className="w-4 h-4" />
          Check for Updates
        </button>
      )}

      {phase === 'checking' && (
        <div className="flex items-center justify-center gap-2 py-2.5 text-theme-text-muted text-sm">
          <Loader2 className="w-4 h-4 animate-spin" />
          Checking for updates...
        </div>
      )}

      {phase === 'up-to-date' && (
        <div className="space-y-3">
          <div className="flex items-center justify-center gap-2 py-2 text-sm text-theme-success">
            <Check className="w-4 h-4" />
            Up to date
          </div>
          <button
            onClick={handleCheck}
            className="btn-ghost w-full flex items-center justify-center gap-2 text-xs"
          >
            <RefreshCw className="w-3 h-3" />
            Check again
          </button>
        </div>
      )}

      {phase === 'available' && updateInfo && (
        <div className="space-y-3">
          <div className="flex items-center justify-center gap-2 text-sm text-theme-text-primary">
            <span className="font-mono text-theme-text-muted">v{updateInfo.installed_version}</span>
            <ArrowRight className="w-3 h-3 text-theme-text-muted" />
            <span className="font-mono text-theme-text-accent">v{updateInfo.available_version}</span>
          </div>
          <button
            onClick={handleApply}
            className="btn-primary w-full flex items-center justify-center gap-2"
          >
            <Download className="w-4 h-4" />
            Apply Update
          </button>
          <p className="text-xs text-theme-text-muted text-center">
            Service will restart automatically
          </p>
        </div>
      )}

      {phase === 'applying' && (
        <div className="flex items-center justify-center gap-2 py-2.5 text-theme-text-muted text-sm">
          <Loader2 className="w-4 h-4 animate-spin" />
          Applying update...
        </div>
      )}

      {phase === 'reconnecting' && (
        <div className="space-y-2 text-center">
          <div className="flex items-center justify-center gap-2 py-2 text-theme-text-muted text-sm">
            <Loader2 className="w-4 h-4 animate-spin" />
            Waiting for restart...
          </div>
          <div className="w-full bg-theme-bg-tertiary rounded-full h-1.5">
            <div
              className="bg-theme-accent h-1.5 rounded-full transition-all duration-500"
              style={{ width: `${Math.min((reconnectAttempts / 30) * 100, 100)}%` }}
            />
          </div>
          <p className="text-xs text-theme-text-muted">
            {reconnectAttempts}s elapsed
          </p>
        </div>
      )}

      {phase === 'updated' && (
        <div className="flex items-center justify-center gap-2 py-2.5 text-theme-success text-sm font-medium">
          <Check className="w-4 h-4" />
          Updated successfully!
        </div>
      )}

      {phase === 'error' && (
        <div className="space-y-3">
          <div className="flex items-center justify-center gap-2 py-2 text-theme-error text-sm">
            <AlertTriangle className="w-4 h-4" />
            {error}
          </div>
          <button
            onClick={() => { setPhase('idle'); setError(null); }}
            className="btn-ghost w-full flex items-center justify-center gap-2 text-xs"
          >
            Try again
          </button>
        </div>
      )}

      {/* Debug Log */}
      {debugLog.length > 0 && (
        <div className="border-t border-theme-border pt-3">
          <button
            onClick={() => setShowDebug(!showDebug)}
            className="btn-ghost flex items-center gap-1 text-xs px-1 py-0.5"
          >
            {showDebug ? <ChevronDown className="w-3 h-3" /> : <ChevronRight className="w-3 h-3" />}
            Debug Log ({debugLog.length})
          </button>
          {showDebug && (
            <pre className="mt-2 p-2 text-[10px] leading-relaxed font-mono bg-theme-bg-tertiary rounded border border-theme-border text-theme-text-muted overflow-x-auto max-h-48 overflow-y-auto whitespace-pre-wrap break-all">
              {debugLog.join('\n')}
            </pre>
          )}
        </div>
      )}
    </div>
  );
}
