/**
 * ProfileManagerDialog — inline expanding card for managing saved APN profiles.
 *
 * Rendered by ConnectionPanel when "Manage Profiles" is clicked. Extracted
 * verbatim from ConnectionPanel.tsx (Item #42 composition root) as a
 * mechanical move; resolves its own modem id via useActiveModemId().
 * Also owns the Import/Export file operations for saved profiles (F4-export).
 */

import { useState, useRef, useEffect } from 'react';
import {
  useActiveModemId,
  useCreateApnProfile, useDeleteApnProfile, useApplyApnProfile, useUpdateApnProfile,
  useImportApnProfiles,
} from '@/hooks';
import { exportApnProfiles } from '@/api';
import { useUIStore } from '@/stores/uiStore';
import {
  Settings, Loader2, AlertTriangle, X, RotateCcw, Save, Trash2, Plus, Eye, EyeOff,
  Upload, Download,
} from 'lucide-react';
import { ResultFeedback } from './ResultFeedback';
import type { ConnectionConfig, AuthType, IpType, ApnProfile } from '@/types/api';

// =============================================================================
// Profile Manager Dialog
// =============================================================================

export interface ProfileManagerDialogProps {
  profiles: ApnProfile[];
  modemProfileId: string;
  onClose: () => void;
}

export function ProfileManagerDialog({
  profiles,
  modemProfileId,
  onClose,
}: ProfileManagerDialogProps) {
  const theme = useUIStore((s) => s.theme);
  const dialogModemId = useActiveModemId();
  const createProfile = useCreateApnProfile();
  const deleteProfile = useDeleteApnProfile();
  const applyProfileMutation = useApplyApnProfile();
  const updateProfile = useUpdateApnProfile();
  const importProfiles = useImportApnProfiles();
  const importFileRef = useRef<HTMLInputElement>(null);
  const [fileOpResult, setFileOpResult] = useState<string | null>(null);
  // Auto-clear timer for fileOpResult. Cleared before every new message so a
  // previous operation's 5 s timer can't wipe a newer message early.
  const fileOpTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [exporting, setExporting] = useState(false);

  // Cancel any pending feedback auto-clear timer on unmount.
  useEffect(() => () => {
    if (fileOpTimerRef.current !== null) clearTimeout(fileOpTimerRef.current);
  }, []);

  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [applyConfirmId, setApplyConfirmId] = useState<string | null>(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editName, setEditName] = useState('');
  const [showCreateForm, setShowCreateForm] = useState(false);
  const [showPassword, setShowPassword] = useState(false);

  // Reset password visibility whenever the create form closes, so reopening it
  // always starts masked — regardless of which close path was taken (X or save).
  useEffect(() => {
    if (!showCreateForm) setShowPassword(false);
  }, [showCreateForm]);

  // Independent form state for creating new profiles
  const [newName, setNewName] = useState('');
  const [newForm, setNewForm] = useState<ConnectionConfig>({
    cid: 1, apn: '', username: '', password: '',
    auth_type: 'none', ip_type: 'ipv4',
  });
  const [newMbn, setNewMbn] = useState('');

  const handleCreate = async () => {
    if (!newName.trim() || !newForm.apn.trim()) return;
    try {
      await createProfile.mutateAsync({
        modemId: dialogModemId!,
        req: {
          name: newName.trim(),
          modem_profile_id: modemProfileId,
          connection: newForm,
          mbn_profile: newMbn.trim() || undefined,
        },
      });
      setNewName('');
      setNewForm({ cid: 1, apn: '', username: '', password: '', auth_type: 'none', ip_type: 'ipv4' });
      setNewMbn('');
      setShowCreateForm(false);
    } catch (e) {
      console.error('Failed to create APN profile:', e);
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await deleteProfile.mutateAsync({ modemId: dialogModemId!, id });
      setConfirmDeleteId(null);
    } catch (e) {
      console.error('Failed to delete APN profile:', e);
    }
  };

  const handleApply = async (id: string) => {
    setApplyConfirmId(null);
    try {
      await applyProfileMutation.mutateAsync({ modemId: dialogModemId!, req: { profile_id: id } });
    } catch (e) {
      console.error('Failed to apply APN profile:', e);
    }
  };

  const handleRename = async (id: string) => {
    if (!editName.trim()) return;
    const profile = profiles.find(p => p.id === id);
    if (!profile) return;
    try {
      await updateProfile.mutateAsync({
        modemId: dialogModemId!,
        id,
        req: {
          name: editName.trim(),
          modem_profile_id: profile.modem_profile_id,
          connection: profile.connection,
          mbn_profile: profile.mbn_profile,
        },
      });
      setEditingId(null);
      setEditName('');
    } catch (e) {
      console.error('Failed to rename APN profile:', e);
    }
  };

  // Import profiles from JSON file (relocated from the panel header — F4-export)
  const handleImportProfiles = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file || !dialogModemId) return;
    // Reset input so the same file can be re-selected
    e.target.value = '';

    try {
      const text = await file.text();
      const parsed = JSON.parse(text);
      const arr = Array.isArray(parsed) ? parsed : [];
      if (arr.length === 0) {
        // Deliberately no auto-clear timer here (spec R3: byte-identical import
        // semantics) — only ensure an earlier timer can't wipe this message.
        if (fileOpTimerRef.current !== null) clearTimeout(fileOpTimerRef.current);
        setFileOpResult('File contains no profiles');
        return;
      }

      // Convert exported ApnProfile[] (with id/timestamps) to ApnProfileRequest[]
      const requests = arr.map((p: Record<string, unknown>) => ({
        name: (p.name as string) || 'Unnamed',
        modem_profile_id: (p.modem_profile_id as string) || modemProfileId,
        connection: (p.connection as ConnectionConfig) || { cid: 1, apn: '', auth_type: 'none', ip_type: 'ipv4' },
        mbn_profile: (p.mbn_profile as string) || undefined,
      }));

      const result = await importProfiles.mutateAsync({ modemId: dialogModemId, profiles: requests });
      if (fileOpTimerRef.current !== null) clearTimeout(fileOpTimerRef.current);
      setFileOpResult(result.message);
      fileOpTimerRef.current = setTimeout(() => setFileOpResult(null), 5000);
    } catch (err) {
      console.error('Failed to import profiles:', err);
      if (fileOpTimerRef.current !== null) clearTimeout(fileOpTimerRef.current);
      setFileOpResult('Failed to parse file — expected JSON array of profiles');
      fileOpTimerRef.current = setTimeout(() => setFileOpResult(null), 5000);
    }
  };

  // Export all saved profiles as pretty-printed JSON (Blob + anchor download).
  // Round-trips through Import unchanged — Import strips ids/timestamps.
  const handleExportProfiles = async () => {
    if (!dialogModemId) return;
    setExporting(true);
    try {
      const exported = await exportApnProfiles(dialogModemId);
      const json = JSON.stringify(exported, null, 2);
      const blob = new Blob([json], { type: 'application/json' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      const now = new Date();
      const date = [
        now.getFullYear(),
        String(now.getMonth() + 1).padStart(2, '0'),
        String(now.getDate()).padStart(2, '0'),
      ].join('-');
      a.href = url;
      a.download = `apn-profiles-${date}.json`;
      document.body.appendChild(a);
      a.click();
      a.remove();
      // Defer revocation — Firefox has historically dropped downloads when the
      // object URL is revoked synchronously after click().
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    } catch (err) {
      console.error('Failed to export profiles:', err);
      if (fileOpTimerRef.current !== null) clearTimeout(fileOpTimerRef.current);
      setFileOpResult('Failed to export profiles');
      fileOpTimerRef.current = setTimeout(() => setFileOpResult(null), 5000);
    } finally {
      setExporting(false);
    }
  };

  const inputClass = 'input-compact w-full';

  const selectClass = 'select-compact w-full';

  return (
    <div className="mt-3 rounded-lg border border-theme-border bg-theme-bg-secondary overflow-hidden">
      {/* Dialog header */}
      <div className="flex items-center justify-between px-3 py-2 border-b border-theme-border bg-theme-bg-tertiary">
        <span className="text-xs font-medium text-theme-text-primary uppercase tracking-wide">
          APN Profiles
        </span>
        <button
          onClick={onClose}
          className="btn-icon p-0.5"
        >
          <X className="w-3.5 h-3.5" />
        </button>
      </div>

      {/* Profile list */}
      <div className="max-h-[16rem] overflow-y-auto">
        {profiles.length === 0 && !showCreateForm ? (
          <div className="px-3 py-4 text-xs text-theme-text-muted text-center">
            No saved profiles yet.
          </div>
        ) : (
          <div className="divide-y divide-theme-border">
            {profiles.map((profile) => (
              <div key={profile.id} className="px-3 py-2">
                {/* Confirm delete banner */}
                {confirmDeleteId === profile.id ? (
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-xs text-theme-error">Delete &ldquo;{profile.name}&rdquo;?</span>
                    <div className="flex items-center gap-1">
                      <button
                        onClick={() => handleDelete(profile.id)}
                        disabled={deleteProfile.isPending}
                        className="btn-danger px-2 py-0.5 text-[10px] bg-theme-error text-white hover:bg-theme-error/90"
                      >
                        {deleteProfile.isPending ? <Loader2 className="w-3 h-3 animate-spin" /> : 'Delete'}
                      </button>
                      <button
                        onClick={() => setConfirmDeleteId(null)}
                        className="btn-secondary px-2 py-0.5 text-[10px]"
                      >
                        Cancel
                      </button>
                    </div>
                  </div>
                ) : applyConfirmId === profile.id ? (
                  /* Apply confirmation */
                  <div className="space-y-1.5">
                    <div className="flex items-start gap-1.5">
                      <AlertTriangle className="w-3 h-3 mt-0.5 shrink-0 text-theme-warning" />
                      <div className="text-[10px] text-theme-text-secondary leading-relaxed">
                        Apply &ldquo;{profile.name}&rdquo;? This will {profile.mbn_profile
                          ? <>select MBN ({profile.mbn_profile})</>
                          : <>enable MBN auto-selection</>
                        }, set APN, and <strong>reboot the modem</strong>.
                      </div>
                    </div>
                    <div className="flex items-center gap-1 justify-end">
                      <button
                        onClick={() => setApplyConfirmId(null)}
                        className="btn-secondary px-2 py-0.5 text-[10px]"
                      >
                        Cancel
                      </button>
                      <button
                        onClick={() => handleApply(profile.id)}
                        disabled={applyProfileMutation.isPending}
                        className="btn-primary flex items-center gap-1 px-2 py-0.5 text-[10px]"
                      >
                        {applyProfileMutation.isPending ? (
                          <Loader2 className="w-2.5 h-2.5 animate-spin" />
                        ) : (
                          <RotateCcw className="w-2.5 h-2.5" />
                        )}
                        Apply & Reboot
                      </button>
                    </div>
                  </div>
                ) : editingId === profile.id ? (
                  /* Rename inline */
                  <div className="flex items-center gap-1.5">
                    <input
                      type="text"
                      value={editName}
                      onChange={(e) => setEditName(e.target.value)}
                      maxLength={50}
                      autoFocus
                      className="input-compact flex-1"
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') handleRename(profile.id);
                        if (e.key === 'Escape') { setEditingId(null); setEditName(''); }
                      }}
                    />
                    <button
                      onClick={() => handleRename(profile.id)}
                      disabled={!editName.trim() || updateProfile.isPending}
                      className="btn-primary px-2 py-1 text-[10px]"
                    >
                      {updateProfile.isPending ? <Loader2 className="w-3 h-3 animate-spin" /> : 'Save'}
                    </button>
                    <button
                      onClick={() => { setEditingId(null); setEditName(''); }}
                      className="btn-icon p-1"
                    >
                      <X className="w-3 h-3" />
                    </button>
                  </div>
                ) : (
                  /* Normal profile row */
                  <div className="flex items-center gap-2">
                    <div className="flex-1 min-w-0">
                      <span className="text-xs font-medium text-theme-text-primary truncate block">
                        {profile.name}
                      </span>
                      <div className="text-[10px] text-theme-text-muted truncate">
                        {profile.connection.apn}
                        {profile.mbn_profile
                          ? <> · MBN: {profile.mbn_profile}</>
                          : <> · MBN: Auto</>
                        }
                      </div>
                    </div>
                    <div className="flex items-center gap-0.5 shrink-0">
                      <button
                        onClick={() => setApplyConfirmId(profile.id)}
                        disabled={applyProfileMutation.isPending}
                        className={`px-1.5 py-0.5 rounded text-[10px] font-medium transition-colors disabled:opacity-40
                                   ${theme === 'fallen'
                                     ? 'text-theme-accent border border-theme-accent/40 hover:bg-theme-accent-muted'
                                     : 'text-theme-accent hover:bg-theme-accent/10'
                                   }`}
                        title={profile.mbn_profile ? 'Apply profile (MBN + APN + reboot)' : 'Apply profile (AutoSel + APN + reboot)'}
                      >
                        Apply
                      </button>
                      <button
                        onClick={() => { setEditingId(profile.id); setEditName(profile.name); }}
                        className="btn-icon p-1"
                        title="Rename"
                      >
                        <Settings className="w-3 h-3" />
                      </button>
                      <button
                        onClick={() => setConfirmDeleteId(profile.id)}
                        className="btn-icon-danger p-1"
                        title="Delete"
                      >
                        <Trash2 className="w-3 h-3" />
                      </button>
                    </div>
                  </div>
                )}
              </div>
            ))}
          </div>
        )}
      </div>

      {/* Apply result feedback inside dialog */}
      {applyProfileMutation.isSuccess && applyProfileMutation.data && (
        <div className="mx-3 mb-2">
          <ResultFeedback
            tone={
              !applyProfileMutation.data.success
                ? 'error'
                : applyProfileMutation.data.had_errors
                  ? 'warning'
                  : 'success'
            }
            title={
              !applyProfileMutation.data.success
                ? 'Failed to apply profile.'
                : applyProfileMutation.data.had_errors
                  ? 'Profile applied with warnings.'
                  : 'Profile applied successfully.'
            }
            stepLog={applyProfileMutation.data.step_log}
            rebooted={applyProfileMutation.data.reboot_triggered}
          />
        </div>
      )}

      {/* Create new profile section */}
      <div className="border-t border-theme-border">
        {showCreateForm ? (
          <div className="px-3 py-2.5 space-y-2">
            <div className="flex items-center justify-between">
              <span className="text-[10px] font-medium text-theme-text-secondary uppercase tracking-wide">
                New Profile
              </span>
              <button
                onClick={() => setShowCreateForm(false)}
                className="p-0.5 text-theme-text-muted hover:text-theme-text-primary"
              >
                <X className="w-3 h-3" />
              </button>
            </div>

            {/* Profile name */}
            <input
              type="text"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              placeholder="Profile name"
              maxLength={50}
              autoFocus
              className={inputClass}
            />

            {/* APN */}
            <input
              type="text"
              value={newForm.apn}
              onChange={(e) => setNewForm(f => ({ ...f, apn: e.target.value }))}
              placeholder="APN (required)"
              className={inputClass}
            />

            {/* Username & Password */}
            <div className="grid grid-cols-2 gap-1.5">
              <input
                type="text"
                value={newForm.username || ''}
                onChange={(e) => setNewForm(f => ({ ...f, username: e.target.value }))}
                placeholder="Username"
                className={inputClass}
              />
              <div className="relative">
                <input
                  type={showPassword ? 'text' : 'password'}
                  value={newForm.password || ''}
                  onChange={(e) => setNewForm(f => ({ ...f, password: e.target.value }))}
                  placeholder="Password"
                  className={`${inputClass} pr-8`}
                  autoComplete="new-password"
                />
                <button
                  type="button"
                  onClick={() => setShowPassword((v) => !v)}
                  className="absolute right-2 top-1/2 -translate-y-1/2 text-theme-text-muted hover:text-theme-text-secondary transition-colors"
                  aria-label={showPassword ? 'Hide password' : 'Show password'}
                >
                  {showPassword ? <EyeOff className="w-3.5 h-3.5" /> : <Eye className="w-3.5 h-3.5" />}
                </button>
              </div>
            </div>

            {/* Auth, IP Type, CID */}
            <div className="grid grid-cols-3 gap-1.5">
              <select
                value={newForm.auth_type}
                onChange={(e) => setNewForm(f => ({ ...f, auth_type: e.target.value as AuthType }))}
                className={selectClass}
              >
                <option value="none">Auth: None</option>
                <option value="pap">Auth: PAP</option>
                <option value="chap">Auth: CHAP</option>
              </select>
              <select
                value={newForm.ip_type}
                onChange={(e) => setNewForm(f => ({ ...f, ip_type: e.target.value as IpType }))}
                className={selectClass}
              >
                <option value="ipv4">IPv4</option>
                <option value="ipv6">IPv6</option>
                <option value="ipv4v6">IPv4v6</option>
              </select>
              <select
                value={newForm.cid}
                onChange={(e) => setNewForm(f => ({ ...f, cid: Number(e.target.value) }))}
                className={selectClass}
              >
                {[1, 2, 3, 4, 5, 6, 7, 8].map(n => (
                  <option key={n} value={n}>CID {n}</option>
                ))}
              </select>
            </div>

            {/* MBN Profile (optional) */}
            <input
              type="text"
              value={newMbn}
              onChange={(e) => setNewMbn(e.target.value)}
              placeholder="MBN profile name (optional, e.g. ROW_Commercial)"
              className={inputClass}
            />

            {/* Save button */}
            <button
              onClick={handleCreate}
              disabled={!newName.trim() || !newForm.apn.trim() || createProfile.isPending}
              className="btn-primary w-full flex items-center justify-center gap-1.5 py-1.5 text-xs"
            >
              {createProfile.isPending ? (
                <Loader2 className="w-3.5 h-3.5 animate-spin" />
              ) : (
                <Save className="w-3.5 h-3.5" />
              )}
              Save Profile
            </button>
          </div>
        ) : (
          <button
            onClick={() => setShowCreateForm(true)}
            className="btn-ghost w-full px-3 py-2 flex items-center justify-center gap-1.5 text-xs"
          >
            <Plus className="w-3.5 h-3.5" />
            New Profile
          </button>
        )}
      </div>

      {/* Import / Export action row (F4-export) */}
      <div className="border-t border-theme-border px-3 py-2 space-y-2">
        <div className="flex items-center gap-1.5">
          <button
            onClick={() => importFileRef.current?.click()}
            disabled={!dialogModemId || importProfiles.isPending}
            className="btn-secondary flex-1 flex items-center justify-center gap-1.5 px-2 py-1 text-[10px]"
            title="Import APN profiles from a JSON file"
          >
            {importProfiles.isPending ? (
              <Loader2 className="w-3 h-3 animate-spin" />
            ) : (
              <Upload className="w-3 h-3" />
            )}
            Import&hellip;
          </button>
          <input
            ref={importFileRef}
            type="file"
            accept=".json"
            className="hidden"
            onChange={handleImportProfiles}
          />
          <button
            onClick={handleExportProfiles}
            disabled={!dialogModemId || profiles.length === 0 || exporting}
            className="btn-secondary flex-1 flex items-center justify-center gap-1.5 px-2 py-1 text-[10px]"
            title={profiles.length === 0 ? 'No saved profiles to export' : 'Download all saved profiles as JSON'}
          >
            {exporting ? (
              <Loader2 className="w-3 h-3 animate-spin" />
            ) : (
              <Download className="w-3 h-3" />
            )}
            Export All
          </button>
        </div>

        {/* Transient import/export feedback — 5 s auto-clear, same pattern as the old header banner */}
        {fileOpResult && (
          <div role="status" className="px-2 py-1.5 rounded text-[10px] bg-theme-accent/10 text-theme-accent">
            {fileOpResult}
          </div>
        )}
      </div>
    </div>
  );
}
