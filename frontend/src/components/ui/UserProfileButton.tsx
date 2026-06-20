/**
 * User Profile Button
 *
 * Header button that opens a popover with:
 * - Username + role display
 * - User Management (admin only) -> opens modal
 * - Change Password -> opens dialog
 * - Sign Out
 *
 * Click-outside-to-close pattern.
 */

import { useState, useRef, useEffect } from 'react';
import { User, Users, KeyRound, LogOut, Shield, Copy, Check } from 'lucide-react';
import clsx from 'clsx';
import { ChangePasswordDialog } from '@/components/auth/ChangePasswordDialog';
import { UserManagementModal } from '@/components/admin/UserManagement';
import type { LicenseStatus } from '@/types/api';

interface UserProfileButtonProps {
  user: { username: string; role: string };
  onLogout: () => void;
  /** When true, show username next to icon and position popover to the right */
  showUsername?: boolean;
  /** License status info to display in popover */
  licenseInfo?: LicenseStatus | null;
}

const ROLE_LABELS: Record<string, string> = {
  read_only: 'Read Only',
  admin: 'Admin',
  super_admin: 'Super Admin',
};

export function UserProfileButton({ user, onLogout, showUsername = false, licenseInfo }: UserProfileButtonProps) {
  const [showPopover, setShowPopover] = useState(false);
  const [showChangePassword, setShowChangePassword] = useState(false);
  const [showUserManagement, setShowUserManagement] = useState(false);
  const [tokenCopied, setTokenCopied] = useState(false);
  const buttonRef = useRef<HTMLButtonElement>(null);

  // Close popover on Escape key
  useEffect(() => {
    if (!showPopover) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setShowPopover(false);
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [showPopover]);

  const isAdmin = user.role === 'admin' || user.role === 'super_admin';

  const handleCopyToken = async () => {
    if (!licenseInfo?.device_token) return;
    try {
      await navigator.clipboard.writeText(licenseInfo.device_token);
      setTokenCopied(true);
      setTimeout(() => setTokenCopied(false), 2000);
    } catch {
      // Fallback: do nothing — clipboard may not be available
    }
  };

  const formatExpiryDate = (iso: string): string => {
    try {
      return new Date(iso).toLocaleDateString('en-US', { month: 'short', day: 'numeric', year: 'numeric' });
    } catch {
      return iso;
    }
  };

  const isExpiringSoon = (iso: string): boolean => {
    try {
      const expiry = new Date(iso).getTime();
      const now = Date.now();
      const thirtyDays = 30 * 24 * 60 * 60 * 1000;
      return expiry - now < thirtyDays && expiry > now;
    } catch {
      return false;
    }
  };

  const licenseStateConfig: Record<string, { color: string; label: string }> = {
    valid: { color: 'bg-theme-success', label: 'Licensed' },
    expired: { color: 'bg-theme-error', label: 'License Expired' },
    invalid_signature: { color: 'bg-theme-warning', label: 'License Invalid' },
    device_mismatch: { color: 'bg-theme-warning', label: 'Device Mismatch' },
  };

  return (
    <>
      <div className="relative shrink-0">
        {/* Profile Button */}
        <button
          ref={buttonRef}
          onClick={() => setShowPopover(!showPopover)}
          className={clsx(
            'flex items-center gap-2 rounded-full transition-colors cursor-pointer',
            'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-theme-accent',
            showUsername
              ? 'px-2 py-1.5 hover:bg-theme-bg-tertiary/50 text-theme-text-secondary hover:text-theme-text-primary w-full'
              : 'justify-center w-8 h-8 sm:w-9 sm:h-9 min-w-[44px] min-h-[44px] sm:min-w-0 sm:min-h-0 bg-theme-bg-tertiary text-theme-text-secondary hover:bg-theme-bg-hover hover:text-theme-text-primary'
          )}
          title={user.username}
        >
          <div className="flex items-center justify-center w-7 h-7 rounded-full bg-theme-bg-tertiary shrink-0">
            <User className="w-3.5 h-3.5" />
          </div>
          {showUsername && (
            <span className="text-sm font-medium truncate">{user.username}</span>
          )}
        </button>

        {/* Profile Modal */}
        {showPopover && (
          <>
            {/* Backdrop */}
            <div
              className="fixed inset-0 bg-black/40 z-50"
              onClick={() => setShowPopover(false)}
            />
            {/* Modal */}
            <div className="fixed inset-0 z-50 flex items-center justify-center pointer-events-none">
              <div
                className="w-80 max-h-[90vh] overflow-y-auto bg-theme-bg-popover border border-theme-border rounded-2xl shadow-lg pointer-events-auto"
                onClick={(e) => e.stopPropagation()}
              >
            {/* User Info */}
            <div className="px-4 py-4">
              <div className="text-base font-medium text-theme-text-primary">{user.username}</div>
              <div className="text-sm text-theme-text-muted">
                Role: {ROLE_LABELS[user.role] || user.role}
              </div>
            </div>

            {/* License Info */}
            {licenseInfo && licenseInfo.state !== 'unlicensed' && (() => {
              const config = licenseStateConfig[licenseInfo.state];
              if (!config) return null;
              return (
                <>
                  <div className="border-t border-theme-border" />
                  <div className="px-4 py-3">
                    {/* Status badge */}
                    <div className="flex items-center gap-2 mb-2">
                      <Shield className="w-4 h-4 text-theme-text-secondary" />
                      <div className="flex items-center gap-1.5">
                        <span className={clsx('w-2 h-2 rounded-full shrink-0', config.color)} />
                        <span className="text-sm font-medium text-theme-text-primary">{config.label}</span>
                      </div>
                    </div>

                    {/* Details */}
                    <div className="space-y-1 ml-6">
                      {licenseInfo.tier && (
                        <div className="text-xs text-theme-text-muted">
                          Tier: {licenseInfo.tier}
                        </div>
                      )}
                      {licenseInfo.expires_at && (
                        <div className={clsx(
                          'text-xs',
                          isExpiringSoon(licenseInfo.expires_at) ? 'text-theme-warning' : 'text-theme-text-muted'
                        )}>
                          Expires: {formatExpiryDate(licenseInfo.expires_at)}
                        </div>
                      )}
                    </div>

                    {/* Device Token */}
                    {licenseInfo.device_token && (
                      <div className="flex items-center gap-2 mt-2.5 ml-6">
                        <span className="text-xs text-theme-text-muted font-mono truncate">
                          {licenseInfo.device_token.substring(0, 8)}...
                        </span>
                        <button
                          type="button"
                          onClick={handleCopyToken}
                          className="shrink-0 flex items-center justify-center w-6 h-6 rounded border border-theme-border bg-theme-bg-tertiary text-theme-text-secondary hover:text-theme-text-primary hover:bg-theme-bg-hover transition-colors"
                          title="Copy device token"
                        >
                          {tokenCopied ? (
                            <Check className="w-3 h-3 text-theme-success" />
                          ) : (
                            <Copy className="w-3 h-3" />
                          )}
                        </button>
                      </div>
                    )}
                  </div>
                </>
              );
            })()}

            <div className="border-t border-theme-border" />

            {/* Menu Items */}
            <div className="p-2">
              {/* User Management — admin only */}
              {isAdmin && (
                <button
                  onClick={() => {
                    setShowPopover(false);
                    setShowUserManagement(true);
                  }}
                  className="w-full flex items-center gap-3 px-4 py-2.5 rounded-lg text-sm text-theme-text-primary hover:bg-theme-bg-tertiary transition-colors text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-theme-accent min-h-[44px] sm:min-h-0"
                >
                  <Users className="w-4 h-4 shrink-0" />
                  <span>User Management</span>
                </button>
              )}

              {/* Change Password */}
              <button
                onClick={() => {
                  setShowPopover(false);
                  setShowChangePassword(true);
                }}
                className="w-full flex items-center gap-3 px-4 py-2.5 rounded-lg text-sm text-theme-text-primary hover:bg-theme-bg-tertiary transition-colors text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-theme-accent min-h-[44px] sm:min-h-0"
              >
                <KeyRound className="w-4 h-4 shrink-0" />
                <span>Change Password</span>
              </button>
            </div>

            <div className="border-t border-theme-border" />

            {/* Sign Out */}
            <div className="p-2">
              <button
                onClick={() => {
                  setShowPopover(false);
                  onLogout();
                }}
                className="w-full flex items-center gap-3 px-4 py-2.5 rounded-lg text-sm text-theme-error hover:bg-theme-bg-tertiary transition-colors text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-theme-accent min-h-[44px] sm:min-h-0"
              >
                <LogOut className="w-4 h-4 shrink-0" />
                <span>Sign Out</span>
              </button>
            </div>
              </div>
            </div>
          </>
        )}
      </div>

      {/* Change Password Dialog */}
      <ChangePasswordDialog
        isOpen={showChangePassword}
        onClose={() => setShowChangePassword(false)}
        username={user.username}
      />

      {/* User Management Modal */}
      <UserManagementModal
        isOpen={showUserManagement}
        onClose={() => setShowUserManagement(false)}
      />
    </>
  );
}
