/**
 * SaveProfileDialog — Item #42 Phase 3
 *
 * Inline name-input strip for saving the current ApnEditor form as a named
 * custom profile. Stateless w.r.t. persistence — the parent owns the create
 * mutation and supplies pending/save/cancel.
 *
 * Design rules:
 *   - OKLCH design tokens only (no hex)
 *   - No emojis
 *   - Label above field, autofocus, Enter to save / Escape to cancel
 */

import { useState } from 'react';
import { Loader2, X } from 'lucide-react';

interface SaveProfileDialogProps {
  /** Called with the trimmed profile name when the user confirms. */
  onSave: (name: string) => void;
  onCancel: () => void;
  isPending: boolean;
}

export function SaveProfileDialog({ onSave, onCancel, isPending }: SaveProfileDialogProps) {
  const [name, setName] = useState('');
  const canSave = name.trim().length > 0 && !isPending;

  function submit() {
    if (!canSave) return;
    onSave(name.trim());
  }

  return (
    <div className="p-3 rounded-lg border border-theme-accent/30 bg-theme-accent/5">
      <label
        htmlFor="apn-save-profile-name"
        className="block text-xs font-semibold text-theme-text-secondary mb-1"
      >
        Profile Name
      </label>
      <div className="flex items-center gap-2">
        <input
          id="apn-save-profile-name"
          type="text"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="e.g., TMO Home Internet"
          maxLength={50}
          autoFocus
          disabled={isPending}
          className="input flex-1 py-1.5"
          onKeyDown={(e) => {
            if (e.key === 'Enter') submit();
            if (e.key === 'Escape') onCancel();
          }}
        />
        <button
          type="button"
          onClick={submit}
          disabled={!canSave}
          className="btn-primary px-3 py-1.5 text-xs"
        >
          {isPending ? <Loader2 className="w-3.5 h-3.5 animate-spin" aria-hidden="true" /> : 'Save'}
        </button>
        <button
          type="button"
          onClick={onCancel}
          disabled={isPending}
          className="btn-icon p-1.5"
          aria-label="Cancel"
        >
          <X className="w-4 h-4" aria-hidden="true" />
        </button>
      </div>
    </div>
  );
}
