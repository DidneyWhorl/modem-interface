/**
 * useWebSocket Hook
 * 
 * Manages WebSocket connection for real-time modem events.
 * Automatically reconnects on disconnect with exponential backoff.
 * Updates React Query cache when events arrive.
 */

import { useEffect, useRef, useCallback, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { createEventSocket } from '@/api/client';
import { fetchWsToken } from '@/api/auth';
import { modemStatusQueryKey, signalQueryKey, simStatusQueryKey, wanStatusQueryKey, speedtestHistoryQueryKey } from '@/hooks/queries';
import { activeProfileQueryKey, detectedModemsQueryKey } from '@/hooks/queries/useModemProfiles';
import { usePageVisibility } from '@/hooks/usePageVisibility';
import { useUIStore } from '@/stores/uiStore';
import type {
  WebSocketEvent,
  SignalInfo,
  ModemStatus,
  ModemHealth,
  WanStatusResponse,
} from '@/types/api';
import type { ActiveModemInfo, DetectedModemEnhanced } from '@/types/profiles';

export type ConnectionStatus = 'connecting' | 'connected' | 'disconnected' | 'error';

export const modemHealthQueryKey = ['modem', 'health'] as const;

interface UseWebSocketOptions {
  /** Enable/disable the WebSocket connection */
  enabled?: boolean;
  /** Callback for all events */
  onEvent?: (event: WebSocketEvent) => void;
  /** Callback for errors */
  onError?: (error: Event) => void;
}

interface UseWebSocketReturn {
  /** Current connection status */
  status: ConnectionStatus;
  /** Last received event */
  lastEvent: WebSocketEvent | null;
  /** Manually reconnect */
  reconnect: () => void;
  /** Manually disconnect */
  disconnect: () => void;
}

const MAX_RECONNECT_DELAY = 30_000; // 30 seconds
const INITIAL_RECONNECT_DELAY = 1_000; // 1 second

export function useWebSocket(options: UseWebSocketOptions = {}): UseWebSocketReturn {
  const { enabled = true, onEvent, onError } = options;

  const isPageVisible = usePageVisibility();
  const queryClient = useQueryClient();
  const wsRef = useRef<WebSocket | null>(null);
  const reconnectTimeoutRef = useRef<number>();
  const reconnectDelayRef = useRef(INITIAL_RECONNECT_DELAY);
  const mountedRef = useRef(true);
  
  // Store callbacks in refs to avoid dependency changes triggering reconnects
  const onEventRef = useRef(onEvent);
  const onErrorRef = useRef(onError);
  
  // Update refs when callbacks change (without triggering reconnect)
  useEffect(() => {
    onEventRef.current = onEvent;
  }, [onEvent]);
  
  useEffect(() => {
    onErrorRef.current = onError;
  }, [onError]);
  
  const [status, setStatus] = useState<ConnectionStatus>('disconnected');
  const [lastEvent, setLastEvent] = useState<WebSocketEvent | null>(null);

  const handleEvent = useCallback((event: WebSocketEvent) => {
    if (!mountedRef.current) return;

    setLastEvent(event);
    onEventRef.current?.(event);

    // Update React Query cache based on event type
    switch (event.type) {
      case 'signal_update': {
        const activeProfile = queryClient.getQueryData<ActiveModemInfo>(activeProfileQueryKey);
        const modems = queryClient.getQueryData<DetectedModemEnhanced[]>(detectedModemsQueryKey);
        const devicePath = activeProfile?.detected?.device_path;
        const activeModemId = devicePath && modems
          ? modems.find(m => m.device_path === devicePath)?.modem_id
          : undefined;

        if (!activeModemId || event.modem_id === activeModemId) {
          queryClient.setQueryData<SignalInfo>(signalQueryKey, event.payload);
        }
        break;
      }
        
      case 'connection_state':
        // Partially update modem status
        queryClient.setQueryData<ModemStatus>(modemStatusQueryKey, (old) => {
          if (!old) return old;
          return {
            ...old,
            connected: event.payload.state === 'connected',
            ip_address: event.payload.ip,
            operator: event.payload.network ?? old.operator,
          };
        });
        break;
        
      case 'registration_change':
        queryClient.setQueryData<ModemStatus>(modemStatusQueryKey, (old) => {
          if (!old) return old;
          return {
            ...old,
            operator: event.payload.operator,
            technology: event.payload.tech,
          };
        });
        break;
        
      case 'sim_event':
        // Invalidate to refetch full SIM status
        queryClient.invalidateQueries({ queryKey: simStatusQueryKey });
        break;

      case 'initial_status':
        // Reconnected — refetch modem status and signal from REST cache.
        // Don't setQueryData here: initial_status payload is { modem_count, modem_ids },
        // not a ModemStatus object. Invalidation triggers a proper GET fetch instead.
        console.log('[WebSocket] Received initial status:', event.payload);
        queryClient.invalidateQueries({ queryKey: modemStatusQueryKey });
        queryClient.invalidateQueries({ queryKey: signalQueryKey });
        break;
        
      case 'modem_health':
        console.log('[WebSocket] Modem health:', event.payload.state, event.payload.message);
        queryClient.setQueryData<ModemHealth>(modemHealthQueryKey, event.payload);

        // Always invalidate modem lists on ANY health change (hot-plug/unplug or recovery)
        queryClient.invalidateQueries({ queryKey: detectedModemsQueryKey });
        queryClient.invalidateQueries({ queryKey: activeProfileQueryKey });

        // When modem comes back online, also refresh all modem data queries
        if (event.payload.available && event.payload.state === 'ok') {
          console.log('[WebSocket] Modem now ok — refreshing all data');
          queryClient.invalidateQueries({ queryKey: ['modem'] });
          queryClient.invalidateQueries({ queryKey: simStatusQueryKey });
        }
        break;

      case 'debug_trace':
        // Dispatch to DebugPanel via custom DOM event (avoids store coupling)
        window.dispatchEvent(new CustomEvent('modem-debug-trace', {
          detail: event.payload.message,
        }));
        break;

      case 'wan_status_update':
        queryClient.setQueryData<WanStatusResponse>(wanStatusQueryKey, event.payload);
        break;

      case 'speedtest_progress':
        // Dispatched as a DOM event so SpeedtestPanel can subscribe without store coupling
        window.dispatchEvent(new CustomEvent('speedtest-progress', { detail: event.payload }));
        break;

      case 'speedtest_complete':
        window.dispatchEvent(new CustomEvent('speedtest-complete', { detail: event.payload }));
        queryClient.invalidateQueries({ queryKey: speedtestHistoryQueryKey });
        break;

      case 'speedtest_error':
        window.dispatchEvent(new CustomEvent('speedtest-error', { detail: event.payload }));
        break;

      case 'error':
        console.error('[WebSocket] Server error:', event.payload.code, event.payload.message);
        break;
    }
  }, [queryClient]); // Only depends on queryClient which is stable

  const disconnectWebSocket = useCallback(() => {
    if (reconnectTimeoutRef.current) {
      clearTimeout(reconnectTimeoutRef.current);
      reconnectTimeoutRef.current = undefined;
    }
    if (wsRef.current) {
      // Prevent onclose from triggering reconnect
      wsRef.current.onclose = null;
      wsRef.current.close();
      wsRef.current = null;
    }
    if (mountedRef.current) {
      setStatus('disconnected');
    }
  }, []);

  const connectWebSocket = useCallback(async () => {
    if (!enabled || !mountedRef.current) {
      return;
    }

    // Don't create new connection if one exists and is open/connecting
    if (wsRef.current && (wsRef.current.readyState === WebSocket.OPEN || wsRef.current.readyState === WebSocket.CONNECTING)) {
      return;
    }

    // Clean up existing connection first
    if (wsRef.current) {
      wsRef.current.onclose = null;
      wsRef.current.close();
      wsRef.current = null;
    }

    setStatus('connecting');

    // Fetch a single-use auth token before opening the WebSocket
    let token: string;
    try {
      console.log('[WebSocket] Fetching auth token...');
      const resp = await fetchWsToken();
      token = resp.token;
    } catch (err) {
      if (!mountedRef.current) return;
      console.error('[WebSocket] Token fetch failed:', err);
      // Schedule reconnect with backoff (e.g. session expired → 401)
      setStatus('disconnected');
      reconnectTimeoutRef.current = window.setTimeout(() => {
        if (mountedRef.current && enabled && !document.hidden) {
          reconnectDelayRef.current = Math.min(
            reconnectDelayRef.current * 2,
            MAX_RECONNECT_DELAY
          );
          connectWebSocket();
        }
      }, reconnectDelayRef.current);
      return;
    }

    if (!mountedRef.current) return;

    console.log('[WebSocket] Connecting...');
    const ws = createEventSocket();
    wsRef.current = ws;

    ws.onopen = () => {
      if (!mountedRef.current) {
        ws.close();
        return;
      }
      console.log('[WebSocket] Connected, sending auth...');
      ws.send(JSON.stringify({ type: 'auth', token }));
      // Stay in 'connecting' until initial_status confirms auth success
    };

    ws.onmessage = (messageEvent) => {
      if (!mountedRef.current) return;
      try {
        const event = JSON.parse(messageEvent.data) as WebSocketEvent;

        // Auth success: server sends initial_status after validating token
        if (event.type === 'initial_status') {
          console.log('[WebSocket] Authenticated');
          setStatus('connected');
          reconnectDelayRef.current = INITIAL_RECONNECT_DELAY;
        }

        // Auth failure: swallow silently — server closes the connection,
        // onclose will trigger reconnect with a fresh token
        if (event.type === 'error' && event.payload?.code === 'auth_failed') {
          return;
        }

        console.log('[WebSocket] Event:', event.type);
        handleEvent(event);
      } catch (err) {
        console.error('[WebSocket] Failed to parse message:', err, messageEvent.data);
      }
    };

    ws.onerror = (errorEvent) => {
      if (!mountedRef.current) return;
      console.error('[WebSocket] Error:', errorEvent);
      setStatus('error');
      onErrorRef.current?.(errorEvent);
    };

    ws.onclose = (closeEvent) => {
      if (!mountedRef.current) return;
      console.log('[WebSocket] Closed:', closeEvent.code, closeEvent.reason);
      setStatus('disconnected');
      wsRef.current = null;

      // Only auto-reconnect if page is visible (use document.hidden directly
      // since the React state may be stale inside this closure)
      if (enabled && mountedRef.current && !document.hidden) {
        console.log(`[WebSocket] Reconnecting in ${reconnectDelayRef.current}ms...`);
        reconnectTimeoutRef.current = window.setTimeout(() => {
          if (mountedRef.current && enabled && !document.hidden) {
            reconnectDelayRef.current = Math.min(
              reconnectDelayRef.current * 2,
              MAX_RECONNECT_DELAY
            );
            connectWebSocket();
          }
        }, reconnectDelayRef.current);
      }
    };
  }, [enabled, handleEvent]); // Minimal stable dependencies

  const reconnect = useCallback(() => {
    disconnectWebSocket();
    reconnectDelayRef.current = INITIAL_RECONNECT_DELAY;
    // Small delay to ensure clean disconnect
    setTimeout(() => {
      if (mountedRef.current) {
        connectWebSocket();
      }
    }, 100);
  }, [connectWebSocket, disconnectWebSocket]);

  // Sync connection status to uiStore so polling hooks can pause
  useEffect(() => {
    useUIStore.getState().setWsConnected(status === 'connected');
  }, [status]);

  // Connect when enabled and page visible, disconnect when hidden or disabled
  useEffect(() => {
    mountedRef.current = true;

    if (enabled && isPageVisible) {
      connectWebSocket();
    } else {
      // Tab hidden or disabled — close WS to free backend resources
      disconnectWebSocket();
    }

    return () => {
      mountedRef.current = false;
      disconnectWebSocket();
    };
  }, [enabled, isPageVisible, connectWebSocket, disconnectWebSocket]);

  return {
    status,
    lastEvent,
    reconnect,
    disconnect: disconnectWebSocket,
  };
}
