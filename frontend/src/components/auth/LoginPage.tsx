/**
 * Login Page
 *
 * Full-page login form with username + password fields.
 * Remembers last username in localStorage.
 */

import { useState, type FormEvent } from 'react';
import { Lock, Eye, EyeOff, AlertCircle, User } from 'lucide-react';
import { AgcccLogo } from '@/components/ui/AgcccLogo';
const LAST_USERNAME_KEY = 'modem-last-username';

interface LoginPageProps {
  onLogin: (username: string, password: string) => Promise<string | null>;
}

export function LoginPage({ onLogin }: LoginPageProps) {
  const [username, setUsername] = useState(() => {
    return localStorage.getItem(LAST_USERNAME_KEY) || 'root';
  });
  const [password, setPassword] = useState('');
  const [showPassword, setShowPassword] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (!username.trim() || !password.trim() || loading) return;

    setError(null);
    setLoading(true);

    try {
      const loginError = await onLogin(username, password);
      if (!loginError) {
        localStorage.setItem(LAST_USERNAME_KEY, username);
      } else {
        setError(loginError);
        setPassword('');
      }
    } catch {
      setError('Invalid credentials');
      setPassword('');
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="min-h-screen bg-theme-bg-primary flex items-center justify-center p-4">
      <div className="w-full max-w-md">
        {/* Logo/Header */}
        <div className="text-center mb-6">
          <div className="inline-flex p-3 bg-theme-bg-tertiary rounded-xl mb-4">
            <AgcccLogo size={32} className="text-theme-text-primary" />
          </div>
          <h1 className="text-xl font-semibold text-theme-text-primary">CTRL-Modem</h1>
          <p className="text-sm text-theme-text-secondary mt-1">
            Cellular Modem Management
          </p>
        </div>

        {/* Login Form */}
        <form
          onSubmit={handleSubmit}
          className="bg-theme-bg-card border border-theme-border rounded-2xl p-8 shadow-lg backdrop-blur-md backdrop-saturate-150"
        >
          <div className="flex items-center gap-2 mb-4">
            <Lock className="w-4 h-4 text-theme-text-secondary" />
            <h2 className="text-sm font-medium text-theme-text-primary">Sign In</h2>
          </div>

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
              placeholder="Username"
              autoComplete="username"
              required
              className="input pl-10 pr-4 py-2.5"
            />
          </div>

          {/* Password field */}
          <div className="relative mb-4">
            <input
              type={showPassword ? 'text' : 'password'}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder="Password"
              autoComplete="current-password"
              autoFocus
              required
              className="input px-4 py-2.5"
            />
            <button
              type="button"
              onClick={() => setShowPassword(!showPassword)}
              className="absolute right-1 top-1/2 -translate-y-1/2 p-2 min-w-[44px] min-h-[44px] sm:min-w-0 sm:min-h-0 sm:p-1 sm:right-3 flex items-center justify-center text-theme-text-muted hover:text-theme-text-secondary rounded"
            >
              {showPassword ? (
                <EyeOff className="w-4 h-4" />
              ) : (
                <Eye className="w-4 h-4" />
              )}
            </button>
          </div>

          <button
            type="submit"
            disabled={loading || !username.trim() || !password.trim()}
            className="btn-primary w-full py-2.5"
          >
            {loading ? 'Signing in...' : 'Sign In'}
          </button>
        </form>
      </div>
    </div>
  );
}
