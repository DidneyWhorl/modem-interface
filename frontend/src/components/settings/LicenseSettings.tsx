/**
 * LicenseSettings Component
 *
 * Opt-in cloud-license panel for the Settings modal.
 *
 * The app is fully usable with no license — this is NOT a wall. It lets an
 * operator who *wants* cloud features (e.g. remote access) view the device
 * token and activate a license key. Reuses the existing license API
 * (getLicenseStatus / activateLicense), the same calls the old full-page
 * LicenseActivationPage used.
 */

import { useCallback, useEffect, useState } from 'react';
import { Cloud, Copy, Check, ChevronDown, Key, ExternalLink, Loader2, AlertCircle } from 'lucide-react';
import clsx from 'clsx';
import { getLicenseStatus, activateLicense } from '@/api/license';
import type { LicenseStatus } from '@/types/api';

interface LicenseSettingsProps {
  /** License info already fetched by the app; used as the initial value. */
  licenseInfo?: LicenseStatus | null;
}

export function LicenseSettings({ licenseInfo }: LicenseSettingsProps) {
  const [status, setStatus] = useState<LicenseStatus | null>(licenseInfo ?? null);
  const [expanded, setExpanded] = useState(false);
  const [licenseKey, setLicenseKey] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [copied, setCopied] = useState(false);

  // Keep a fresh copy of license status (device token + current state).
  useEffect(() => {
    let cancelled = false;
    getLicenseStatus()
      .then((s) => { if (!cancelled) setStatus(s); })
      .catch(() => { /* silent — license is optional */ });
    return () => { cancelled = true; };
  }, []);

  const isLicensed = status?.state === 'valid';

  const handleCopy = useCallback(async () => {
    if (!status?.device_token) return;
    try {
      await navigator.clipboard.writeText(status.device_token);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      const el = document.getElementById('license-device-token');
      if (el) {
        const range = document.createRange();
        range.selectNodeContents(el);
        const selection = window.getSelection();
        selection?.removeAllRanges();
        selection?.addRange(range);
      }
    }
  }, [status]);

  const handleActivate = useCallback(async () => {
    const trimmed = licenseKey.trim();
    if (!trimmed || loading) return;

    setError(null);
    setLoading(true);
    try {
      const result = await activateLicense(trimmed);
      setStatus(result);
      if (result.state === 'valid') {
        setLicenseKey('');
      } else {
        const messages: Record<string, string> = {
          expired: 'This license has expired.',
          invalid_signature: 'Invalid license key. Please check and try again.',
          device_mismatch: 'This license was issued for a different device.',
          unlicensed: 'Activation failed. Please check your license key.',
        };
        setError(messages[result.state] || 'Activation failed.');
      }
    } catch (err: unknown) {
      const message = typeof err === 'object' && err !== null && 'message' in err
        ? String((err as { message: unknown }).message)
        : null;
      setError(message || 'Activation failed. Please try again.');
    } finally {
      setLoading(false);
    }
  }, [licenseKey, loading]);

  return (
    <div>
      <button
        onClick={() => setExpanded((v) => !v)}
        className={clsx(
          'w-full flex items-center gap-2 px-3 py-2.5 sm:py-2 min-h-[44px] sm:min-h-0',
          'rounded-lg transition-colors text-sm',
          'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-theme-accent focus-visible:ring-offset-1',
          isLicensed
            ? 'text-theme-success bg-theme-success/20 hover:bg-theme-success/30'
            : 'text-theme-text-secondary hover:text-theme-text-primary hover:bg-theme-bg-tertiary/50'
        )}
        title="Cloud license (optional — unlocks cloud features)"
      >
        <Cloud className="w-4 h-4" />
        <span className="flex-1 text-left">
          {isLicensed ? 'Cloud Features: Active' : 'Cloud Features: Not Activated'}
        </span>
        <ChevronDown className={clsx('w-4 h-4 transition-transform', expanded && 'rotate-180')} />
      </button>

      {expanded && (
        <div className="px-1 pt-2 space-y-3">
          <p className="text-[10px] text-theme-text-muted leading-relaxed">
            Local modem management works without a license. Activate a license
            only if you want optional cloud features (e.g. remote access).
          </p>

          {/* Error */}
          {error && (
            <div className="flex items-start gap-2 p-2 rounded-lg bg-theme-error/10 border border-theme-error/20 text-theme-error text-xs">
              <AlertCircle className="w-3.5 h-3.5 flex-shrink-0 mt-0.5" />
              <span>{error}</span>
            </div>
          )}

          {/* Device Token */}
          {status?.device_token && (
            <div>
              <label className="block text-[10px] font-medium text-theme-text-secondary mb-1">
                Device Token
              </label>
              <div className="flex gap-2">
                <div
                  id="license-device-token"
                  className="flex-1 font-mono text-[10px] bg-theme-bg-tertiary border border-theme-border rounded-lg px-2 py-1.5 text-theme-text-primary break-all select-all overflow-auto max-h-16"
                >
                  {status.device_token}
                </div>
                <button
                  type="button"
                  onClick={handleCopy}
                  className="shrink-0 flex items-center justify-center w-8 h-8 rounded-lg border border-theme-border bg-theme-bg-tertiary text-theme-text-secondary hover:text-theme-text-primary hover:bg-theme-bg-hover transition-colors"
                  title="Copy device token"
                >
                  {copied ? (
                    <Check className="w-3.5 h-3.5 text-theme-success" />
                  ) : (
                    <Copy className="w-3.5 h-3.5" />
                  )}
                </button>
              </div>
            </div>
          )}

          {/* License Key Input */}
          {!isLicensed && (
            <div>
              <label className="block text-[10px] font-medium text-theme-text-secondary mb-1">
                <span className="flex items-center gap-1.5">
                  <Key className="w-3 h-3" />
                  <span>License Key</span>
                </span>
              </label>
              <textarea
                value={licenseKey}
                onChange={(e) => setLicenseKey(e.target.value)}
                placeholder="Paste your license key here..."
                rows={3}
                className="input w-full px-2 py-1.5 text-xs font-mono resize-none"
              />
              <button
                type="button"
                onClick={handleActivate}
                disabled={loading || !licenseKey.trim()}
                className="btn-primary w-full py-2 mt-2 flex items-center justify-center gap-2 text-xs"
              >
                {loading ? (
                  <>
                    <Loader2 className="w-3.5 h-3.5 animate-spin" />
                    <span>Activating...</span>
                  </>
                ) : (
                  'Activate'
                )}
              </button>

              <a
                href="https://portal.ctrl-modem.com"
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex items-center gap-1.5 text-[10px] text-theme-accent hover:underline mt-2"
              >
                <span>Register your device to get a license key</span>
                <ExternalLink className="w-3 h-3" />
              </a>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
