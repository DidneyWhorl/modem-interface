/**
 * Change Password Dialog
 *
 * Modal for authenticated users to change their own password.
 * Root users see a message directing them to SSH `passwd` instead.
 * Shows animated 3-2-1 countdown on success before auto-closing.
 */

import { useState, useEffect, useRef, type FormEvent } from 'react';
import { X, Lock, AlertCircle, CheckCircle2 } from 'lucide-react';
import { changePassword } from '@/api/auth';
import { ApiClientError } from '@/api/client';
interface ChangePasswordDialogProps {
  isOpen: boolean;
  onClose: () => void;
  username: string;
}

export function ChangePasswordDialog({ isOpen, onClose, username }: ChangePasswordDialogProps) {
  const [currentPassword, setCurrentPassword] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState(false);
  const [countdown, setCountdown] = useState(0);
  const [loading, setLoading] = useState(false);
  const countdownRef = useRef<ReturnType<typeof setInterval> | null>(null);

  // Cleanup interval on unmount
  useEffect(() => {
    return () => {
      if (countdownRef.current) clearInterval(countdownRef.current);
    };
  }, []);

  if (!isOpen) return null;

  const isRoot = username === 'root';

  const startCountdown = () => {
    setCountdown(3);
    let count = 3;
    countdownRef.current = setInterval(() => {
      count -= 1;
      if (count <= 0) {
        if (countdownRef.current) clearInterval(countdownRef.current);
        countdownRef.current = null;
        setSuccess(false);
        setCountdown(0);
        onClose();
      } else {
        setCountdown(count);
      }
    }, 1000);
  };

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (loading) return;

    setError(null);

    if (newPassword.length < 4) {
      setError('New password must be at least 4 characters');
      return;
    }

    if (newPassword !== confirmPassword) {
      setError('Passwords do not match');
      return;
    }

    setLoading(true);
    try {
      const result = await changePassword(currentPassword, newPassword);
      if (result.success) {
        setSuccess(true);
        setCurrentPassword('');
        setNewPassword('');
        setConfirmPassword('');
        startCountdown();
      }
    } catch (err) {
      if (err instanceof ApiClientError) {
        // Surface backend error message (e.g. "Current password is incorrect")
        setError(err.message);
      } else {
        setError('Failed to change password');
      }
    } finally {
      setLoading(false);
    }
  };

  const handleClose = () => {
    if (countdownRef.current) {
      clearInterval(countdownRef.current);
      countdownRef.current = null;
    }
    setCurrentPassword('');
    setNewPassword('');
    setConfirmPassword('');
    setError(null);
    setSuccess(false);
    setCountdown(0);
    onClose();
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50" onClick={handleClose}>
      <div
        className="bg-theme-bg-popover border border-theme-border rounded-2xl p-8 shadow-xl w-full max-w-lg mx-4"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between mb-4">
          <div className="flex items-center gap-2">
            <Lock className="w-4 h-4 text-theme-text-secondary" />
            <h2 className="text-sm font-medium text-theme-text-primary">Change Password</h2>
          </div>
          <button
            onClick={handleClose}
            className="btn-icon p-2.5 sm:p-1 min-w-[44px] min-h-[44px] sm:min-w-0 sm:min-h-0 flex items-center justify-center"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        {isRoot ? (
          <div className="text-sm text-theme-text-secondary">
            <p className="mb-2">
              The root password is managed by OpenWRT and cannot be changed from this interface.
            </p>
            <p>
              To change the root password, SSH into the router and run:
            </p>
            <code className="block mt-2 p-2 bg-theme-bg-tertiary rounded text-xs font-mono text-theme-text-primary">
              passwd
            </code>
          </div>
        ) : success ? (
          /* Success with animated countdown */
          <div className="text-center py-4">
            <CheckCircle2 className="w-10 h-10 text-theme-success mx-auto mb-3" />
            <p className="text-sm font-medium text-theme-success mb-4">
              Password changed successfully!
            </p>
            <div className="relative mx-auto w-16 h-16 mb-3">
              {/* Circular countdown animation */}
              <svg className="w-16 h-16 -rotate-90" viewBox="0 0 64 64">
                <circle
                  cx="32" cy="32" r="28"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="3"
                  className="text-theme-bg-tertiary"
                />
                <circle
                  cx="32" cy="32" r="28"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="3"
                  strokeDasharray={2 * Math.PI * 28}
                  strokeDashoffset={2 * Math.PI * 28 * (1 - countdown / 3)}
                  strokeLinecap="round"
                  className="text-theme-success transition-all duration-1000 ease-linear"
                />
              </svg>
              <span className="absolute inset-0 flex items-center justify-center text-2xl font-bold text-theme-text-primary">
                {countdown}
              </span>
            </div>
            <p className="text-xs text-theme-text-muted">
              Closing automatically...
            </p>
          </div>
        ) : (
          <form onSubmit={handleSubmit}>
            {error && (
              <div className="flex items-center gap-2 mb-4 p-3 rounded-lg bg-theme-error/10 border border-theme-error/20 text-theme-error text-sm">
                <AlertCircle className="w-4 h-4 flex-shrink-0" />
                {error}
              </div>
            )}

            <input
              type="password"
              value={currentPassword}
              onChange={(e) => setCurrentPassword(e.target.value)}
              placeholder="Current password"
              autoComplete="current-password"
              required
              className="input px-4 py-2.5 mb-3"
            />

            <input
              type="password"
              value={newPassword}
              onChange={(e) => setNewPassword(e.target.value)}
              placeholder="New password"
              autoComplete="new-password"
              required
              className="input px-4 py-2.5 mb-3"
            />

            <input
              type="password"
              value={confirmPassword}
              onChange={(e) => setConfirmPassword(e.target.value)}
              placeholder="Confirm new password"
              autoComplete="new-password"
              required
              className="input px-4 py-2.5 mb-4"
            />

            <button
              type="submit"
              disabled={loading || !currentPassword || !newPassword || !confirmPassword}
              className="btn-primary w-full py-2.5"
            >
              {loading ? 'Changing...' : 'Change Password'}
            </button>
          </form>
        )}
      </div>
    </div>
  );
}
