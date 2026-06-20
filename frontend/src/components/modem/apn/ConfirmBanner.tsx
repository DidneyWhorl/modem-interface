/**
 * ConfirmBanner
 *
 * Inline confirmation strip used for:
 *   - MBN change warning (modem will reboot)
 *   - Reconnect with dirty form (unsaved edits will not apply)
 */

import { AlertTriangle, X, Loader2 } from 'lucide-react';

interface ConfirmBannerProps {
  title: string;
  body: string;
  confirmLabel: string;
  onConfirm: () => void;
  onCancel: () => void;
  isPending?: boolean;
  variant?: 'warning' | 'caution';
}

export function ConfirmBanner({
  title,
  body,
  confirmLabel,
  onConfirm,
  onCancel,
  isPending = false,
  variant = 'warning',
}: ConfirmBannerProps) {
  const colorClass =
    variant === 'warning'
      ? 'bg-theme-warning/10 border-theme-warning/30'
      : 'bg-theme-error/10 border-theme-error/30';
  const iconClass = variant === 'warning' ? 'text-theme-warning' : 'text-theme-error';
  const confirmClass =
    variant === 'warning'
      ? 'bg-theme-warning text-white hover:opacity-90'
      : 'bg-theme-error text-white hover:opacity-90';

  return (
    <div
      className={`rounded-lg border p-3 space-y-2.5 ${colorClass}`}
      role="alertdialog"
      aria-modal="false"
      aria-labelledby="confirm-banner-title"
      aria-describedby="confirm-banner-body"
    >
      <div className="flex items-start gap-2">
        <AlertTriangle className={`w-3.5 h-3.5 mt-0.5 shrink-0 ${iconClass}`} aria-hidden="true" />
        <div className="flex-1 text-xs">
          <p id="confirm-banner-title" className="font-semibold text-theme-text-primary">{title}</p>
          <p id="confirm-banner-body" className="text-theme-text-secondary mt-1 text-caption leading-relaxed">{body}</p>
        </div>
        <button
          type="button"
          onClick={onCancel}
          className="p-0.5 text-theme-text-muted hover:text-theme-text-primary transition-colors"
          aria-label="Dismiss"
        >
          <X className="w-3 h-3" />
        </button>
      </div>

      <div className="flex items-center gap-2 justify-end">
        <button
          type="button"
          onClick={onCancel}
          disabled={isPending}
          className="px-3 py-1.5 rounded-md text-caption font-medium text-theme-text-secondary
                     hover:bg-theme-bg-tertiary transition-colors
                     disabled:opacity-50 disabled:cursor-not-allowed"
        >
          Cancel
        </button>
        <button
          type="button"
          onClick={onConfirm}
          disabled={isPending}
          className={`flex items-center gap-1.5 px-3 py-1.5 rounded-md text-caption font-medium
                      transition-colors active:scale-[0.98]
                      disabled:opacity-50 disabled:cursor-not-allowed ${confirmClass}`}
        >
          {isPending && <Loader2 className="w-3 h-3 animate-spin" />}
          {confirmLabel}
        </button>
      </div>
    </div>
  );
}
