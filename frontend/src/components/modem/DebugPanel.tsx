/**
 * DebugPanel — Real-time debug trace log from the backend.
 *
 * Shows every AT command sent/received, reconnect watcher steps,
 * APN enforcement decisions, WWAN bounces, MBN operations, etc.
 * Entries arrive via WebSocket `debug_trace` events, dispatched
 * as DOM CustomEvents by the useWebSocket hook.
 */

import { useState, useEffect, useRef, useCallback } from 'react';
import { Trash2, Pause, Play, Download, ScrollText } from 'lucide-react';

interface DebugEntry {
  id: number;
  timestamp: string;
  message: string;
}

const MAX_ENTRIES = 500;
let nextId = 0;

export function DebugPanel() {
  const [entries, setEntries] = useState<DebugEntry[]>([]);
  const [paused, setPaused] = useState(false);
  const [filter, setFilter] = useState('');
  const scrollRef = useRef<HTMLDivElement>(null);
  const pausedRef = useRef(false);
  const bufferRef = useRef<DebugEntry[]>([]);

  // Keep ref in sync for use in event listener
  pausedRef.current = paused;

  useEffect(() => {
    const handler = (e: Event) => {
      const msg = (e as CustomEvent<string>).detail;
      const entry: DebugEntry = {
        id: nextId++,
        timestamp: new Date().toLocaleTimeString('en-GB', {
          hour12: false,
          hour: '2-digit',
          minute: '2-digit',
          second: '2-digit',
          fractionalSecondDigits: 3,
        } as Intl.DateTimeFormatOptions),
        message: msg,
      };

      if (pausedRef.current) {
        // Buffer while paused so we don't lose entries
        bufferRef.current.push(entry);
        if (bufferRef.current.length > MAX_ENTRIES) {
          bufferRef.current = bufferRef.current.slice(-MAX_ENTRIES);
        }
      } else {
        setEntries(prev => {
          const next = [...prev, entry];
          return next.length > MAX_ENTRIES ? next.slice(-MAX_ENTRIES) : next;
        });
      }
    };

    window.addEventListener('modem-debug-trace', handler);
    return () => window.removeEventListener('modem-debug-trace', handler);
  }, []);

  // Flush buffer when unpausing
  useEffect(() => {
    if (!paused && bufferRef.current.length > 0) {
      setEntries(prev => {
        const merged = [...prev, ...bufferRef.current];
        bufferRef.current = [];
        return merged.length > MAX_ENTRIES ? merged.slice(-MAX_ENTRIES) : merged;
      });
    }
  }, [paused]);

  // Auto-scroll to bottom when new entries arrive (unless paused)
  useEffect(() => {
    if (!paused && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [entries, paused]);

  const handleClear = useCallback(() => {
    setEntries([]);
    bufferRef.current = [];
  }, []);

  const handleExport = useCallback(() => {
    const text = entries.map(e => `${e.timestamp}  ${e.message}`).join('\n');
    const blob = new Blob([text], { type: 'text/plain' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `modem-debug-${new Date().toISOString().slice(0, 19).replace(/:/g, '-')}.log`;
    a.click();
    URL.revokeObjectURL(url);
  }, [entries]);

  const filtered = filter
    ? entries.filter(e => e.message.toLowerCase().includes(filter.toLowerCase()))
    : entries;

  return (
    <div className="h-full flex flex-col gap-2">
      {/* Toolbar */}
      <div className="flex items-center gap-2 shrink-0">
        <input
          type="text"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="Filter..."
          className="input-compact flex-1"
        />
        <button
          onClick={() => setPaused(!paused)}
          title={paused ? 'Resume' : 'Pause'}
          className="btn-icon p-1"
        >
          {paused ? <Play className="w-3.5 h-3.5" /> : <Pause className="w-3.5 h-3.5" />}
        </button>
        <button
          onClick={handleExport}
          title="Export log"
          className="btn-icon p-1"
        >
          <Download className="w-3.5 h-3.5" />
        </button>
        <button
          onClick={handleClear}
          title="Clear"
          className="btn-icon p-1"
        >
          <Trash2 className="w-3.5 h-3.5" />
        </button>
        <span className="text-[10px] text-theme-text-muted tabular-nums">
          {filtered.length}{paused ? ' (paused)' : ''}
        </span>
      </div>

      {/* Log entries */}
      <div
        ref={scrollRef}
        className="flex-1 min-h-0 overflow-y-auto font-mono text-caption leading-relaxed
                   bg-theme-bg-primary rounded border border-theme-border p-1.5"
      >
        {filtered.length === 0 ? (
          <div className="empty-state py-4">
            <ScrollText className="w-6 h-6 text-theme-text-muted" />
            <p className="text-xs text-theme-text-secondary">
              {filter ? 'No entries match your filter' : 'Waiting for debug traces'}
            </p>
            <p className="text-[10px] text-theme-text-muted">
              {filter ? 'Try a different search term' : 'Debug events will appear here as they occur'}
            </p>
          </div>
        ) : (
          filtered.map((entry) => (
            <div key={entry.id} className="flex gap-2 hover:bg-theme-bg-secondary/50 px-1 rounded">
              <span className="text-theme-text-muted shrink-0 select-none">
                {entry.timestamp}
              </span>
              <span className={getMessageColor(entry.message)}>
                {entry.message}
              </span>
            </div>
          ))
        )}
      </div>
    </div>
  );
}

/** Color-code messages by prefix/content for readability. */
function getMessageColor(msg: string): string {
  if (msg.startsWith('AT TX')) return 'text-theme-accent';
  if (msg.startsWith('AT RX ←')) return 'text-theme-success';
  if (msg.startsWith('AT RX ✗')) return 'text-theme-error';
  if (msg.includes('ERROR') || msg.includes('failed') || msg.includes('MISMATCH'))
    return 'text-theme-error';
  if (msg.includes('[APN-ENFORCE]')) return 'text-theme-warning';
  if (msg.includes('[RECONNECT]')) return 'text-theme-text-accent';
  if (msg.includes('[MBN]')) return 'text-theme-text-accent';
  if (msg.includes('[WWAN]')) return 'text-theme-warning';
  return 'text-theme-text-primary';
}
