/**
 * ProfileRequestDialog Component
 *
 * Modal dialog for requesting a modem profile from the developer.
 * Provides both a mailto: link and a copy-to-clipboard fallback.
 */

import { useState } from 'react';
import { X, Mail, Copy, Check, ExternalLink } from 'lucide-react';
import { useRequestProfile } from '@/hooks/queries';
import type { ProfileRequestPayload } from '@/types/profiles';

interface Props {
  isOpen: boolean;
  onClose: () => void;
  vendorId: string;
  productId: string;
  deviceInfoResponse: string;
}

export function ProfileRequestDialog({
  isOpen,
  onClose,
  vendorId,
  productId,
  deviceInfoResponse,
}: Props) {
  const [notes, setNotes] = useState('');
  const [copied, setCopied] = useState(false);
  const requestMutation = useRequestProfile();

  if (!isOpen) return null;

  const handleSubmit = () => {
    const payload: ProfileRequestPayload = {
      vendor_id: vendorId,
      product_id: productId,
      device_info_response: deviceInfoResponse,
      user_notes: notes,
    };
    requestMutation.mutate(payload);
  };

  const handleCopy = async () => {
    if (requestMutation.data?.request_text) {
      await navigator.clipboard.writeText(requestMutation.data.request_text);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }
  };

  const handleMailto = () => {
    if (requestMutation.data?.mailto_link) {
      window.open(requestMutation.data.mailto_link, '_blank');
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
      <div className="bg-theme-bg-primary border border-theme-border rounded-lg shadow-xl w-full max-w-lg mx-4">
        {/* Header */}
        <div className="flex items-center justify-between px-5 py-3 border-b border-theme-border">
          <h3 className="text-lg font-medium text-theme-text-primary">
            Request Modem Profile
          </h3>
          <button
            onClick={onClose}
            className="p-1 text-theme-text-muted hover:text-theme-text-primary"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        {/* Content */}
        <div className="px-5 py-4 space-y-4">
          {!requestMutation.data ? (
            <>
              <p className="text-sm text-theme-text-secondary">
                No specific profile exists for this modem. Request one from the
                developer and we'll add support for your hardware.
              </p>

              {/* Auto-filled info */}
              <div className="space-y-2">
                <div className="flex gap-4 text-sm">
                  <div>
                    <span className="text-theme-text-muted">Vendor: </span>
                    <span className="font-mono text-theme-text-primary">{vendorId || 'Unknown'}</span>
                  </div>
                  <div>
                    <span className="text-theme-text-muted">Product: </span>
                    <span className="font-mono text-theme-text-primary">{productId || 'Unknown'}</span>
                  </div>
                </div>

                {deviceInfoResponse && (
                  <div>
                    <label className="text-xs text-theme-text-muted block mb-1">
                      Device Info (ATI response)
                    </label>
                    <pre className="text-xs font-mono bg-theme-bg-tertiary rounded p-2 text-theme-text-secondary max-h-20 overflow-y-auto">
                      {deviceInfoResponse}
                    </pre>
                  </div>
                )}
              </div>

              {/* User notes */}
              <div>
                <label className="text-sm text-theme-text-secondary block mb-1">
                  Additional notes (optional)
                </label>
                <textarea
                  value={notes}
                  onChange={(e) => setNotes(e.target.value)}
                  placeholder="e.g., carrier, region, specific features needed..."
                  className="w-full px-3 py-2 text-sm rounded border border-theme-border bg-theme-bg-secondary text-theme-text-primary placeholder:text-theme-text-muted resize-none"
                  rows={3}
                />
              </div>

              <button
                onClick={handleSubmit}
                disabled={requestMutation.isPending}
                className="w-full py-2 px-4 bg-theme-accent text-white rounded font-medium text-sm hover:opacity-90 transition-opacity disabled:opacity-50"
              >
                {requestMutation.isPending ? 'Generating...' : 'Generate Request'}
              </button>
            </>
          ) : (
            <>
              <p className="text-sm text-theme-success">
                Profile request generated. Use one of the options below to send it:
              </p>

              {/* Action buttons */}
              <div className="flex gap-3">
                <button
                  onClick={handleMailto}
                  className="flex-1 flex items-center justify-center gap-2 py-2.5 px-4 bg-theme-accent text-white rounded font-medium text-sm hover:opacity-90 transition-opacity"
                >
                  <Mail className="w-4 h-4" />
                  Open Email Client
                  <ExternalLink className="w-3 h-3" />
                </button>

                <button
                  onClick={handleCopy}
                  className="flex items-center gap-2 py-2.5 px-4 border border-theme-border rounded font-medium text-sm text-theme-text-secondary hover:bg-theme-bg-secondary transition-colors"
                >
                  {copied ? (
                    <>
                      <Check className="w-4 h-4 text-theme-success" />
                      Copied
                    </>
                  ) : (
                    <>
                      <Copy className="w-4 h-4" />
                      Copy
                    </>
                  )}
                </button>
              </div>

              <div className="text-xs text-theme-text-muted">
                Send to: <span className="font-mono">modem.requests@netsolution.shop</span>
              </div>

              {/* Preview */}
              <details className="text-sm">
                <summary className="text-theme-text-muted cursor-pointer hover:text-theme-text-secondary">
                  Preview request text
                </summary>
                <pre className="mt-2 text-xs font-mono bg-theme-bg-tertiary rounded p-3 text-theme-text-secondary max-h-40 overflow-y-auto whitespace-pre-wrap">
                  {requestMutation.data.request_text}
                </pre>
              </details>
            </>
          )}

          {requestMutation.error && (
            <p className="text-sm text-theme-error">
              Error: {requestMutation.error.message}
            </p>
          )}
        </div>
      </div>
    </div>
  );
}
