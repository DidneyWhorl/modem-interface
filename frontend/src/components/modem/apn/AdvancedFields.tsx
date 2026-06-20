/**
 * AdvancedFields
 *
 * Collapsed-by-default advanced section: username + password with show/hide.
 * Password placeholder rule: when has_password is true and the user has not
 * typed anything, userPasswordEdited=false and the input shows a masked
 * placeholder. The parent must NOT include the masked value in the request.
 */

import { useState } from 'react';
import { ChevronDown, ChevronUp, Settings, Eye, EyeOff } from 'lucide-react';

const MASKED_PLACEHOLDER = '••••••••';

export interface AdvancedFieldValues {
  username: string;
  /** Raw typed value. Empty string = cleared. MASKED_PLACEHOLDER = not edited. */
  password: string;
  /** true when the user has typed in the password field this session. */
  passwordEdited: boolean;
}

interface AdvancedFieldsProps {
  values: AdvancedFieldValues;
  hasStoredPassword: boolean;
  onChange: (next: Partial<AdvancedFieldValues>) => void;
  disabled: boolean;
}

export function AdvancedFields({
  values,
  hasStoredPassword,
  onChange,
  disabled,
}: AdvancedFieldsProps) {
  const [open, setOpen] = useState(false);
  const [showPassword, setShowPassword] = useState(false);

  // Derive the displayed password value:
  // - If the user hasn't edited it yet and a stored password exists, show masked placeholder
  // - Otherwise show what the user typed (including empty string)
  const displayedPassword =
    !values.passwordEdited && hasStoredPassword ? MASKED_PLACEHOLDER : values.password;

  function handlePasswordChange(raw: string) {
    onChange({
      password: raw === MASKED_PLACEHOLDER ? '' : raw,
      passwordEdited: true,
    });
  }

  function handlePasswordFocus() {
    // When the user focuses the masked placeholder, clear it so they can type fresh
    if (!values.passwordEdited && hasStoredPassword) {
      onChange({ password: '', passwordEdited: true });
    }
  }

  return (
    <div>
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex items-center gap-1.5 text-xs text-theme-text-secondary hover:text-theme-text-primary transition-colors"
        aria-expanded={open}
        aria-controls="apn-advanced-fields"
      >
        {open ? <ChevronUp className="w-3.5 h-3.5" /> : <ChevronDown className="w-3.5 h-3.5" />}
        <Settings className="w-3.5 h-3.5" />
        <span className="font-semibold">Advanced Options</span>
      </button>

      {open && (
        <div id="apn-advanced-fields" className="mt-3 space-y-3">
          {/* Username */}
          <div>
            <label
              htmlFor="apn-editor-username"
              className="block text-sm font-semibold text-theme-text-secondary mb-1"
            >
              Username
            </label>
            <input
              id="apn-editor-username"
              type="text"
              value={values.username}
              onChange={(e) => onChange({ username: e.target.value })}
              placeholder="Optional"
              disabled={disabled}
              className="input"
              autoComplete="off"
            />
          </div>

          {/* Password */}
          <div>
            <label
              htmlFor="apn-editor-password"
              className="block text-sm font-semibold text-theme-text-secondary mb-1"
            >
              Password
            </label>
            <div className="relative">
              <input
                id="apn-editor-password"
                type={showPassword ? 'text' : 'password'}
                value={displayedPassword}
                onChange={(e) => handlePasswordChange(e.target.value)}
                onFocus={handlePasswordFocus}
                placeholder="Optional"
                disabled={disabled}
                className="input pr-10"
                autoComplete="new-password"
                aria-describedby={hasStoredPassword && !values.passwordEdited ? 'apn-password-hint' : undefined}
              />
              <button
                type="button"
                onClick={() => setShowPassword((v) => !v)}
                className="absolute right-2 top-1/2 -translate-y-1/2 text-theme-text-muted hover:text-theme-text-secondary transition-colors"
                aria-label={showPassword ? 'Hide password' : 'Show password'}
              >
                {showPassword ? <EyeOff className="w-4 h-4" /> : <Eye className="w-4 h-4" />}
              </button>
            </div>
            {hasStoredPassword && !values.passwordEdited && (
              <p id="apn-password-hint" className="mt-1 text-caption text-theme-text-muted">
                A password is stored on the modem. Click to replace or clear it.
              </p>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
