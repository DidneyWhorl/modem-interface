/**
 * ATTerminal Component
 *
 * Interactive terminal for sending AT commands to the modem.
 * Features:
 * - Command input with submit
 * - Command history (up/down arrows)
 * - Response display with syntax highlighting
 * - Confirmation prompts for privileged commands
 * - Clear history option
 */

import { useState, useRef, useEffect, KeyboardEvent } from 'react';
import { useATCommand } from '@/hooks';
import { useUIStore } from '@/stores/uiStore';
import { useCurrentUser } from '@/contexts/UserContext';
import { ApiClientError } from '@/api/client';
import { Terminal, Send, Trash2, AlertTriangle, ShieldAlert, ShieldCheck, Settings2 } from 'lucide-react';
import { WhitelistManager } from './WhitelistManager';

interface PendingConfirmation {
  command: string;
  reason: string;
}

export function ATTerminal() {
  const [command, setCommand] = useState('');
  const [historyIndex, setHistoryIndex] = useState(-1);
  const [pendingConfirmation, setPendingConfirmation] = useState<PendingConfirmation | null>(null);
  const [showWhitelistManager, setShowWhitelistManager] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const outputRef = useRef<HTMLDivElement>(null);

  const currentUser = useCurrentUser();
  const { mutate: executeCommand, isPending } = useATCommand();

  // Whitelist manager visible to Admin+ with at-whitelist feature permission
  const canManageWhitelist = currentUser && (
    currentUser.role === 'admin' || currentUser.role === 'super_admin'
  ) && (
    currentUser.allowedFeatures === null || currentUser.allowedFeatures.includes('at-whitelist')
  );

  const {
    atHistory,
    atInputHistory,
    addATHistoryEntry,
    addATInputHistory,
    clearATHistory,
  } = useUIStore();

  // Auto-scroll to bottom when history updates
  useEffect(() => {
    if (outputRef.current) {
      outputRef.current.scrollTop = outputRef.current.scrollHeight;
    }
  }, [atHistory, pendingConfirmation]);

  const handleExecute = (cmd: string, confirmed: boolean = false) => {
    executeCommand(
      { command: cmd, confirmed },
      {
        onSuccess: (response) => {
          addATHistoryEntry({
            command: cmd,
            response: response.response,
            timestamp: Date.now(),
            success: response.success,
          });
          setPendingConfirmation(null);
        },
        onError: (error) => {
          if (error instanceof ApiClientError) {
            // 428 = Precondition Required (needs confirmation)
            if (error.status === 428) {
              setPendingConfirmation({
                command: cmd,
                reason: error.message || 'This command requires confirmation',
              });
              return;
            }

            // 403 = Forbidden (blocked command)
            if (error.status === 403) {
              addATHistoryEntry({
                command: cmd,
                response: `BLOCKED: ${error.message}`,
                timestamp: Date.now(),
                success: false,
              });
              return;
            }
          }

          // Other errors
          addATHistoryEntry({
            command: cmd,
            response: `Error: ${error.message}`,
            timestamp: Date.now(),
            success: false,
          });
        },
      }
    );
  };

  const handleSubmit = () => {
    const trimmed = command.trim();
    if (!trimmed || isPending) return;

    // Add to input history
    addATInputHistory(trimmed);
    setHistoryIndex(-1);

    // Execute without confirmation first
    handleExecute(trimmed, false);

    setCommand('');
  };

  const handleConfirm = () => {
    if (!pendingConfirmation) return;
    handleExecute(pendingConfirmation.command, true);
  };

  const handleCancelConfirmation = () => {
    if (pendingConfirmation) {
      addATHistoryEntry({
        command: pendingConfirmation.command,
        response: 'Cancelled by user',
        timestamp: Date.now(),
        success: false,
      });
    }
    setPendingConfirmation(null);
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      handleSubmit();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (atInputHistory.length === 0) return;

      const newIndex = historyIndex < atInputHistory.length - 1
        ? historyIndex + 1
        : historyIndex;
      setHistoryIndex(newIndex);
      setCommand(atInputHistory[atInputHistory.length - 1 - newIndex] || '');
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (historyIndex <= 0) {
        setHistoryIndex(-1);
        setCommand('');
      } else {
        const newIndex = historyIndex - 1;
        setHistoryIndex(newIndex);
        setCommand(atInputHistory[atInputHistory.length - 1 - newIndex] || '');
      }
    }
  };

  const formatResponse = (response: string): string => {
    // Clean up response for display
    return response
      .replace(/\r\n/g, '\n')
      .replace(/\r/g, '\n')
      .trim();
  };

  return (
    <div className="card overflow-hidden h-full flex flex-col">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 bg-theme-bg-secondary border-b border-theme-border shrink-0">
        <div className="flex items-center gap-2">
          <Terminal className="w-5 h-5 text-theme-text-accent" />
          <h2 className="text-sm font-medium text-theme-text-primary">AT Terminal</h2>
        </div>
        <div className="flex items-center gap-1">
          {canManageWhitelist && (
            <button
              onClick={() => setShowWhitelistManager(true)}
              className="btn-icon p-1.5"
              title="Manage AT command whitelist"
            >
              <Settings2 className="w-4 h-4" />
            </button>
          )}
          <button
            onClick={clearATHistory}
            className="btn-icon p-1.5"
            title="Clear history"
          >
            <Trash2 className="w-4 h-4" />
          </button>
        </div>
      </div>

      {/* Output Area */}
      <div
        ref={outputRef}
        className="flex-1 min-h-0 overflow-y-auto bg-theme-bg-primary p-4 font-mono text-sm scrollbar-hide"
      >
        {atHistory.length === 0 && !pendingConfirmation ? (
          <div className="empty-state py-6">
            <Terminal className="w-8 h-8 text-theme-text-muted" />
            <p className="text-sm text-theme-text-secondary">No commands sent yet</p>
            <div className="text-xs text-theme-text-muted font-mono text-left mt-2 space-y-0.5">
              <p>AT&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;- Basic test</p>
              <p>ATI&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;- Device info</p>
              <p>AT+CSQ&nbsp;&nbsp;&nbsp;&nbsp;- Signal strength</p>
              <p>AT+COPS?&nbsp;&nbsp;- Current operator</p>
              <p>AT+CPIN?&nbsp;&nbsp;- SIM status</p>
            </div>
            <p className="text-[10px] text-theme-text-muted mt-2">
              Some commands may require confirmation or be blocked for safety.
            </p>
          </div>
        ) : (
          <div className="space-y-3">
            {atHistory.map((entry, i) => (
              <div key={i}>
                {/* Command */}
                <div className="flex items-start gap-2">
                  <span className="text-theme-text-accent select-none">&gt;</span>
                  <span className="text-theme-text-accent">{entry.command}</span>
                </div>
                {/* Response */}
                <div
                  className={`ml-4 mt-1 whitespace-pre-wrap break-all ${
                    entry.response.startsWith('BLOCKED')
                      ? 'text-theme-error'
                      : entry.response.startsWith('Cancelled')
                      ? 'text-theme-warning'
                      : entry.success
                      ? 'text-theme-text-secondary'
                      : 'text-theme-error'
                  }`}
                >
                  {formatResponse(entry.response)}
                </div>
              </div>
            ))}

            {/* Pending Confirmation */}
            {pendingConfirmation && (
              <div className="mt-4 p-3 bg-theme-warning/10 border border-theme-warning/30 rounded-lg">
                <div className="flex items-start gap-2 mb-2">
                  <ShieldAlert className="w-5 h-5 text-theme-warning flex-shrink-0 mt-0.5" />
                  <div>
                    <div className="text-theme-warning font-medium">
                      Confirmation Required
                    </div>
                    <div className="text-theme-warning/80 text-xs mt-1">
                      {pendingConfirmation.reason}
                    </div>
                    <div className="text-theme-text-muted mt-2">
                      Command: <span className="text-theme-text-accent">{pendingConfirmation.command}</span>
                    </div>
                  </div>
                </div>
                <div className="flex gap-2 mt-3 ml-7">
                  <button
                    onClick={handleConfirm}
                    disabled={isPending}
                    className="btn-warning flex items-center gap-1.5 px-3 py-1.5 text-xs"
                  >
                    <ShieldCheck className="w-3.5 h-3.5" />
                    Confirm & Execute
                  </button>
                  <button
                    onClick={handleCancelConfirmation}
                    disabled={isPending}
                    className="btn-secondary px-3 py-1.5 text-xs"
                  >
                    Cancel
                  </button>
                </div>
              </div>
            )}
          </div>
        )}
      </div>

      {/* Warning Banner */}
      <div className="px-4 py-2 bg-theme-bg-secondary/50 border-t border-theme-border/50 shrink-0">
        <div className="flex items-center gap-2 text-xs text-theme-text-muted">
          <AlertTriangle className="w-3.5 h-3.5" />
          <span>
            Commands not in the safe list will require confirmation. Some are blocked entirely.
          </span>
        </div>
      </div>

      {/* Input Area */}
      <div className="flex items-center gap-2 px-4 py-3 bg-theme-bg-secondary border-t border-theme-border shrink-0">
        <span className="text-theme-text-accent font-mono select-none">&gt;</span>
        <input
          ref={inputRef}
          type="text"
          value={command}
          onChange={(e) => setCommand(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder={pendingConfirmation ? "Respond to confirmation above..." : "Enter AT command..."}
          disabled={isPending || !!pendingConfirmation}
          className="flex-1 bg-transparent text-theme-text-accent font-mono text-sm placeholder-theme-text-muted outline-none focus-visible:ring-2 focus-visible:ring-theme-accent focus-visible:ring-offset-1 focus-visible:ring-offset-theme-bg-primary disabled:opacity-50"
          autoComplete="off"
          spellCheck={false}
        />
        <button
          onClick={handleSubmit}
          disabled={isPending || !command.trim() || !!pendingConfirmation}
          className="btn-icon p-2 text-theme-text-accent"
        >
          {isPending ? (
            <div className="loading-spinner-sm !h-4 !w-4" />
          ) : (
            <Send className="w-4 h-4" />
          )}
        </button>
      </div>

      {/* Whitelist Manager Modal */}
      {canManageWhitelist && (
        <WhitelistManager
          isOpen={showWhitelistManager}
          onClose={() => setShowWhitelistManager(false)}
        />
      )}
    </div>
  );
}
