/**
 * Setup Page
 *
 * First-run account creation page.
 * Creates an admin account with a username and password.
 * Root user can always log in with the router's SSH password.
 */

import { useState, type FormEvent } from 'react';
import { ShieldCheck, Eye, EyeOff, AlertCircle, Check, User } from 'lucide-react';
import { AgcccLogo } from '@/components/ui/AgcccLogo';
import clsx from 'clsx';

interface SetupPageProps {
  onSetup: (username: string, password: string) => Promise<boolean>;
}

export function SetupPage({ onSetup }: SetupPageProps) {
  const [username, setUsername] = useState('admin');
  const [password, setPassword] = useState('');
  const [confirm, setConfirm] = useState('');
  const [showPassword, setShowPassword] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const passwordsMatch = password.length > 0 && password === confirm;
  const tooShort = password.length > 0 && password.length < 4;

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (loading) return;

    if (!username.trim()) {
      setError('Username is required');
      return;
    }
    if (password.length < 4) {
      setError('Password must be at least 4 characters');
      return;
    }
    if (password !== confirm) {
      setError('Passwords do not match');
      return;
    }

    setError(null);
    setLoading(true);

    const success = await onSetup(username, password);
    if (!success) {
      setError('Failed to create account. Please try again.');
    }
    setLoading(false);
  };

  return (
    <div className="min-h-screen bg-theme-bg-primary flex items-center justify-center p-4">
      <div className="w-full max-w-md">
        {/* Logo/Header */}
        <div className="text-center mb-8">
          <div className="inline-flex p-3 bg-theme-bg-tertiary rounded-xl mb-4">
            <AgcccLogo size={32} className="text-theme-text-primary" />
          </div>
          <h1 className="text-xl font-semibold text-theme-text-primary">CTRL-Modem</h1>
          <p className="text-sm text-theme-text-secondary mt-1">
            First-Time Setup
          </p>
        </div>

        {/* Setup Form */}
        <form
          onSubmit={handleSubmit}
          className="bg-theme-bg-card border border-theme-border rounded-2xl p-8 shadow-lg backdrop-blur-md backdrop-saturate-150"
        >
          <div className="flex items-center gap-2 mb-2">
            <ShieldCheck className="w-4 h-4 text-theme-text-secondary" />
            <h2 className="text-sm font-medium text-theme-text-primary">Create Admin Account</h2>
          </div>
          <p className="text-xs text-theme-text-muted mb-4">
            Create an admin account to manage the interface. The router's root
            user can also log in using its SSH password.
          </p>

          {error && (
            <div className="flex items-center gap-2 mb-4 p-3 rounded-lg bg-theme-error/10 border border-theme-error/20 text-theme-error text-sm">
              <AlertCircle className="w-4 h-4 flex-shrink-0" />
              {error}
            </div>
          )}

          {/* Username field */}
          <div className="relative mb-3">
            <div className="absolute left-3 top-1/2 -translate-y-1/2 text-theme-text-muted">
              <User className="w-4 h-4" />
            </div>
            <input
              type="text"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              placeholder="Admin username"
              autoComplete="username"
              required
              className="input pl-10 pr-4 py-2.5"
            />
          </div>

          {/* Password field */}
          <div className="relative mb-3">
            <input
              type={showPassword ? 'text' : 'password'}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder="Password"
              autoFocus
              required
              minLength={4}
              className={clsx(
                'input px-4 py-2.5',
                tooShort && 'border-theme-error/50 focus-visible:border-theme-error'
              )}
            />
            <button
              type="button"
              onClick={() => setShowPassword(!showPassword)}
              className="btn-icon absolute right-1 top-1/2 -translate-y-1/2 p-2 min-w-[44px] min-h-[44px] sm:min-w-0 sm:min-h-0 sm:p-1 sm:right-3 flex items-center justify-center hover:text-theme-text-secondary"
            >
              {showPassword ? (
                <EyeOff className="w-4 h-4" />
              ) : (
                <Eye className="w-4 h-4" />
              )}
            </button>
          </div>

          {tooShort && (
            <p className="text-xs text-theme-error mb-3">Minimum 4 characters</p>
          )}

          {/* Confirm field */}
          <div className="relative mb-4">
            <input
              type={showPassword ? 'text' : 'password'}
              value={confirm}
              onChange={(e) => setConfirm(e.target.value)}
              placeholder="Confirm password"
              required
              className={clsx(
                'input px-4 py-2.5',
                confirm.length > 0 && !passwordsMatch && 'border-theme-error/50 focus-visible:border-theme-error'
              )}
            />
            {passwordsMatch && (
              <Check className="absolute right-3 top-1/2 -translate-y-1/2 w-4 h-4 text-theme-success" />
            )}
          </div>

          <button
            type="submit"
            disabled={loading || !passwordsMatch || tooShort || !username.trim()}
            className="btn-primary w-full py-2.5"
          >
            {loading ? 'Setting up...' : 'Create Account'}
          </button>
        </form>

        <p className="text-xs text-theme-text-muted text-center mt-4">
          You can reset passwords via SSH: modem-interface --reset-password &lt;user&gt; &lt;pass&gt;
        </p>
      </div>
    </div>
  );
}
