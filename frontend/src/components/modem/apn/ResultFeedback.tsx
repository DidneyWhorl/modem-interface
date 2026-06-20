/**
 * ResultFeedback
 *
 * Presentation-only feedback primitive for the result of an Apply, Reconnect,
 * or saved-profile-apply action. Renders a tinted banner (success / warning /
 * error chrome) with a title, an optional reboot notice, and an optional
 * step log. Callers decode their own result shape into these primitive props.
 */

import { Check, AlertTriangle, RotateCcw } from 'lucide-react';

export type ResultTone = 'success' | 'warning' | 'error';

interface ResultFeedbackProps {
  tone: ResultTone;
  title: string;
  stepLog: string[];
  rebooted: boolean;
}

const TONE_CLASS: Record<ResultTone, string> = {
  success: 'bg-theme-success/10 border-theme-success/30 text-theme-success',
  warning: 'bg-theme-warning/10 border-theme-warning/30 text-theme-warning',
  error: 'bg-theme-error/10 border-theme-error/30 text-theme-error',
};

export function ResultFeedback({ tone, title, stepLog, rebooted }: ResultFeedbackProps) {
  const outerClass = TONE_CLASS[tone];
  const Icon = tone === 'success' ? Check : AlertTriangle;

  return (
    <div className={`rounded-lg border p-3 space-y-2 text-xs ${outerClass}`} role="status">
      <div className="flex items-start gap-2">
        <Icon className="w-3.5 h-3.5 mt-0.5 shrink-0" aria-hidden="true" />
        <p className="font-semibold">{title}</p>
      </div>

      {rebooted && (
        <div className="flex items-center gap-1.5 text-theme-warning font-medium pl-5">
          <RotateCcw className="w-3 h-3" aria-hidden="true" />
          <span>The modem is rebooting and will reconnect automatically.</span>
        </div>
      )}

      {stepLog.length > 0 && (
        <ul className="pl-5 space-y-0.5 text-caption opacity-80 list-none">
          {stepLog.map((step, i) => (
            <li key={i} className="text-theme-text-secondary">
              {step}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
