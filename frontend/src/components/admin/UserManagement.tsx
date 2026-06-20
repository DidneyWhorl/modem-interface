/**
 * User Management Panel
 *
 * Admin panel for managing user accounts. Visible to Admin and SuperAdmin roles.
 * - List users with role/status
 * - Create, edit, delete users
 * - Reset passwords
 * - Set panel permissions for ReadOnly users
 *
 * Role restrictions:
 * - SuperAdmin: full CRUD, can assign any role
 * - Admin: can only manage ReadOnly users, cannot assign Admin or SuperAdmin
 * - Users cannot disable their own account
 */

import { useState, useEffect, useCallback, type FormEvent } from 'react';
import { Plus, Trash2, KeyRound, Shield, ShieldAlert, Eye, UserX, UserCheck, AlertCircle, AlertTriangle, X } from 'lucide-react';
import { listUsers, createUser, updateUser, deleteUser, resetUserPassword } from '@/api/users';
import type { UserInfo, CreateUserRequest, UpdateUserRequest } from '@/api/users';
import { PANEL_CONFIGS } from '@/stores/uiStore';
import { ApiClientError } from '@/api/client';
import { useCurrentUser } from '@/contexts/UserContext';

const ALL_ROLE_OPTIONS = ['read_only', 'admin', 'super_admin'] as const;
const ROLE_LABELS: Record<string, string> = {
  read_only: 'Read Only',
  admin: 'Admin',
  super_admin: 'Super Admin',
};
const ROLE_ICONS: Record<string, React.ElementType> = {
  read_only: Eye,
  admin: Shield,
  super_admin: ShieldAlert,
};

/** Get available role options based on the caller's role. */
function getRoleOptions(callerRole: string | undefined): string[] {
  if (callerRole === 'super_admin') {
    return [...ALL_ROLE_OPTIONS];
  }
  // Admins and below cannot create/assign super_admin
  return ALL_ROLE_OPTIONS.filter(r => r !== 'super_admin');
}

export function UserManagement() {
  const currentUser = useCurrentUser();
  const [users, setUsers] = useState<UserInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showCreateForm, setShowCreateForm] = useState(false);
  const [editingUser, setEditingUser] = useState<string | null>(null);
  const [resetPasswordUser, setResetPasswordUser] = useState<string | null>(null);

  const roleOptions = getRoleOptions(currentUser?.role);

  const fetchUsers = useCallback(async () => {
    try {
      setError(null);
      const data = await listUsers();
      setUsers(data);
    } catch (err) {
      if (err instanceof ApiClientError) {
        setError(err.message);
      } else {
        setError('Failed to load users');
      }
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchUsers();
  }, [fetchUsers]);

  if (loading) {
    return (
      <div className="loading-state">
        <div className="loading-spinner" />
        <span>Loading users...</span>
      </div>
    );
  }

  if (error && users.length === 0) {
    return (
      <div className="p-4">
        <div className="error-state">
          <AlertTriangle className="w-8 h-8 text-theme-error" />
          <p className="text-sm text-theme-text-secondary">Failed to load user list</p>
          <p className="text-xs text-theme-text-muted">{error}</p>
          <button onClick={fetchUsers} className="btn-ghost text-xs">Try again</button>
        </div>
      </div>
    );
  }

  return (
    <div className="p-4 space-y-3">
      {error && (
        <div className="flex items-center gap-2 p-3 rounded-lg bg-theme-error/10 border border-theme-error/20 text-theme-error text-sm">
          <AlertCircle className="w-4 h-4 flex-shrink-0" />
          {error}
        </div>
      )}

      {/* User List */}
      <div className="space-y-2">
        {users.map((u) => (
          <UserRow
            key={u.username}
            user={u}
            currentUsername={currentUser?.username}
            roleOptions={roleOptions}
            isEditing={editingUser === u.username}
            isResettingPassword={resetPasswordUser === u.username}
            onEdit={() => setEditingUser(editingUser === u.username ? null : u.username)}
            onCancelEdit={() => setEditingUser(null)}
            onUpdate={async (req) => {
              await updateUser(u.username, req);
              setEditingUser(null);
              fetchUsers();
            }}
            onDelete={async () => {
              if (confirm(`Delete user "${u.username}"?`)) {
                await deleteUser(u.username);
                fetchUsers();
              }
            }}
            onResetPassword={() =>
              setResetPasswordUser(resetPasswordUser === u.username ? null : u.username)
            }
            onSubmitResetPassword={async (newPassword) => {
              await resetUserPassword(u.username, newPassword);
              setResetPasswordUser(null);
            }}
            onCancelResetPassword={() => setResetPasswordUser(null)}
          />
        ))}
      </div>

      {/* Create User */}
      {showCreateForm ? (
        <CreateUserForm
          roleOptions={roleOptions}
          onSubmit={async (req) => {
            await createUser(req);
            setShowCreateForm(false);
            fetchUsers();
          }}
          onCancel={() => setShowCreateForm(false)}
        />
      ) : (
        <button
          onClick={() => setShowCreateForm(true)}
          className="btn-ghost w-full flex items-center justify-center gap-2 border border-dashed border-theme-border hover:border-theme-accent"
        >
          <Plus className="w-4 h-4" />
          Add User
        </button>
      )}
    </div>
  );
}

// --- User Row ---

interface UserRowProps {
  user: UserInfo;
  currentUsername: string | undefined;
  roleOptions: string[];
  isEditing: boolean;
  isResettingPassword: boolean;
  onEdit: () => void;
  onCancelEdit: () => void;
  onUpdate: (req: UpdateUserRequest) => Promise<void>;
  onDelete: () => Promise<void>;
  onResetPassword: () => void;
  onSubmitResetPassword: (password: string) => Promise<void>;
  onCancelResetPassword: () => void;
}

function UserRow({
  user,
  currentUsername,
  roleOptions,
  isEditing,
  isResettingPassword,
  onEdit,
  onCancelEdit,
  onUpdate,
  onDelete,
  onResetPassword,
  onSubmitResetPassword,
  onCancelResetPassword,
}: UserRowProps) {
  const RoleIcon = ROLE_ICONS[user.role] || Eye;
  const isRoot = user.username === 'root';
  const isSelf = user.username === currentUsername;

  return (
    <div className="bg-theme-bg-secondary rounded-lg border border-theme-border overflow-hidden">
      {/* Main row */}
      <div className="flex items-center gap-3 px-3 py-2">
        <RoleIcon className="w-4 h-4 text-theme-text-secondary flex-shrink-0" />
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2">
            <span className="text-sm font-medium text-theme-text-primary truncate">
              {user.username}
            </span>
            {user.disabled && (
              <span className="text-[10px] px-1.5 py-0.5 rounded bg-theme-error/20 text-theme-error">
                disabled
              </span>
            )}
            {isRoot && (
              <span className="text-[10px] px-1.5 py-0.5 rounded bg-theme-accent-muted text-theme-text-accent">
                system
              </span>
            )}
            {isSelf && (
              <span className="text-[10px] px-1.5 py-0.5 rounded bg-theme-accent/20 text-theme-accent">
                you
              </span>
            )}
          </div>
          <span className="text-xs text-theme-text-muted">{ROLE_LABELS[user.role] || user.role}</span>
        </div>

        {!isRoot && (
          <div className="flex items-center gap-1">
            <button
              onClick={onResetPassword}
              className="btn-icon p-1.5 hover:text-theme-warning hover:bg-theme-warning/10"
              title="Reset password"
            >
              <KeyRound className="w-3.5 h-3.5" />
            </button>
            <button
              onClick={onEdit}
              className="btn-icon p-1.5"
              title="Edit user"
            >
              {user.disabled ? (
                <UserCheck className="w-3.5 h-3.5" />
              ) : (
                <UserX className="w-3.5 h-3.5" />
              )}
            </button>
            {!isSelf && (
              <button
                onClick={onDelete}
                className="btn-icon-danger p-1.5"
                title="Delete user"
              >
                <Trash2 className="w-3.5 h-3.5" />
              </button>
            )}
          </div>
        )}
      </div>

      {/* Edit panel */}
      {isEditing && !isRoot && (
        <EditUserForm
          user={user}
          isSelf={isSelf}
          roleOptions={roleOptions}
          onUpdate={onUpdate}
          onCancel={onCancelEdit}
        />
      )}

      {/* Reset password panel */}
      {isResettingPassword && !isRoot && (
        <ResetPasswordForm onSubmit={onSubmitResetPassword} onCancel={onCancelResetPassword} />
      )}
    </div>
  );
}

// --- Edit User Form ---

function EditUserForm({
  user,
  isSelf,
  roleOptions,
  onUpdate,
  onCancel,
}: {
  user: UserInfo;
  isSelf: boolean;
  roleOptions: string[];
  onUpdate: (req: UpdateUserRequest) => Promise<void>;
  onCancel: () => void;
}) {
  const [role, setRole] = useState(user.role);
  const [disabled, setDisabled] = useState(user.disabled);
  const [allowedPanels, setAllowedPanels] = useState<string[] | null>(user.allowed_panels);
  const [allowedFeatures, setAllowedFeatures] = useState<string[] | null>(user.allowed_features);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setLoading(true);
    setError(null);
    try {
      await onUpdate({ role, allowed_panels: allowedPanels, allowed_features: allowedFeatures, disabled });
    } catch (err) {
      if (err instanceof ApiClientError) {
        setError(err.message);
      } else {
        setError('Failed to update user');
      }
    } finally {
      setLoading(false);
    }
  };

  const togglePanel = (panelId: string) => {
    if (!allowedPanels) {
      // Currently "all panels" — switch to all except this one
      setAllowedPanels(
        PANEL_CONFIGS.filter((p) => p.id !== panelId).map((p) => p.id)
      );
    } else if (allowedPanels.includes(panelId)) {
      setAllowedPanels(allowedPanels.filter((p) => p !== panelId));
    } else {
      setAllowedPanels([...allowedPanels, panelId]);
    }
  };

  const allPanelsEnabled = !allowedPanels;

  return (
    <form onSubmit={handleSubmit} className="px-3 pb-3 pt-1 border-t border-theme-border space-y-3">
      {error && (
        <div className="flex items-center gap-2 p-2 rounded bg-theme-error/10 text-theme-error text-xs">
          <AlertCircle className="w-3 h-3 flex-shrink-0" />
          {error}
        </div>
      )}

      {/* Role */}
      <div>
        <label className="text-xs text-theme-text-muted block mb-1">Role</label>
        <select
          value={role}
          onChange={(e) => setRole(e.target.value)}
          className="select w-full py-1.5"
        >
          {roleOptions.map((r) => (
            <option key={r} value={r}>
              {ROLE_LABELS[r]}
            </option>
          ))}
        </select>
      </div>

      {/* Disabled toggle — cannot disable yourself */}
      <label className="flex items-center gap-2 text-sm text-theme-text-secondary cursor-pointer">
        <input
          type="checkbox"
          checked={disabled}
          onChange={(e) => setDisabled(e.target.checked)}
          disabled={isSelf}
          className="rounded"
        />
        <span className={isSelf ? 'opacity-50' : ''}>
          Account disabled
          {isSelf && <span className="text-xs text-theme-text-muted ml-1">(cannot disable yourself)</span>}
        </span>
      </label>

      {/* Panel permissions (only for read_only users) */}
      {role === 'read_only' && (
        <div>
          <div className="flex items-center justify-between mb-1">
            <label className="text-xs text-theme-text-muted">Allowed Panels</label>
            <button
              type="button"
              onClick={() => setAllowedPanels(allPanelsEnabled ? [] : null)}
              className="text-[10px] text-theme-accent hover:underline"
            >
              {allPanelsEnabled ? 'Restrict' : 'Allow All'}
            </button>
          </div>
          <div className="grid grid-cols-2 gap-1">
            {PANEL_CONFIGS.map((panel) => (
              <label
                key={panel.id}
                className="flex items-center gap-1.5 text-xs text-theme-text-secondary cursor-pointer"
              >
                <input
                  type="checkbox"
                  checked={allPanelsEnabled || (allowedPanels?.includes(panel.id) ?? false)}
                  onChange={() => togglePanel(panel.id)}
                  disabled={allPanelsEnabled}
                  className="rounded"
                />
                {panel.title}
              </label>
            ))}
          </div>
        </div>
      )}

      {/* Feature permissions (for admin and super_admin users) */}
      {(role === 'admin' || role === 'super_admin') && (
        <div>
          <div className="flex items-center justify-between mb-1">
            <label className="text-xs text-theme-text-muted">Feature Permissions</label>
            <button
              type="button"
              onClick={() => setAllowedFeatures(allowedFeatures === null ? [] : null)}
              className="text-[10px] text-theme-accent hover:underline"
            >
              {allowedFeatures === null ? 'Restrict' : 'Allow All'}
            </button>
          </div>
          <label className="flex items-center gap-1.5 text-xs text-theme-text-secondary cursor-pointer">
            <input
              type="checkbox"
              checked={allowedFeatures === null || allowedFeatures.includes('at-whitelist')}
              onChange={() => {
                if (allowedFeatures === null) {
                  // Switch from unrestricted to restricting this feature
                  setAllowedFeatures([]);
                } else if (allowedFeatures.includes('at-whitelist')) {
                  setAllowedFeatures(allowedFeatures.filter(f => f !== 'at-whitelist'));
                } else {
                  setAllowedFeatures([...allowedFeatures, 'at-whitelist']);
                }
              }}
              disabled={allowedFeatures === null}
              className="rounded"
            />
            AT Whitelist Manager
          </label>
        </div>
      )}

      <div className="flex gap-2">
        <button
          type="submit"
          disabled={loading}
          className="btn-primary flex-1 py-1.5 text-xs"
        >
          {loading ? 'Saving...' : 'Save'}
        </button>
        <button
          type="button"
          onClick={onCancel}
          className="btn-secondary flex-1 py-1.5 text-xs"
        >
          Cancel
        </button>
      </div>
    </form>
  );
}

// --- Reset Password Form ---

function ResetPasswordForm({
  onSubmit,
  onCancel,
}: {
  onSubmit: (password: string) => Promise<void>;
  onCancel: () => void;
}) {
  const [password, setPassword] = useState('');
  const [confirm, setConfirm] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (password.length < 4) {
      setError('Password must be at least 4 characters');
      return;
    }
    if (password !== confirm) {
      setError('Passwords do not match');
      return;
    }
    setLoading(true);
    setError(null);
    try {
      await onSubmit(password);
    } catch (err) {
      if (err instanceof ApiClientError) {
        setError(err.message);
      } else {
        setError('Failed to reset password');
      }
    } finally {
      setLoading(false);
    }
  };

  return (
    <form onSubmit={handleSubmit} className="px-3 pb-3 pt-1 border-t border-theme-border space-y-2">
      {error && (
        <div className="flex items-center gap-2 p-2 rounded bg-theme-error/10 text-theme-error text-xs">
          <AlertCircle className="w-3 h-3 flex-shrink-0" />
          {error}
        </div>
      )}
      <input
        type="password"
        value={password}
        onChange={(e) => setPassword(e.target.value)}
        placeholder="New password"
        autoComplete="new-password"
        required
        className="input py-1.5"
      />
      <input
        type="password"
        value={confirm}
        onChange={(e) => setConfirm(e.target.value)}
        placeholder="Confirm password"
        autoComplete="new-password"
        required
        className="input py-1.5"
      />
      <div className="flex gap-2">
        <button
          type="submit"
          disabled={loading}
          className="btn-warning flex-1 py-1.5 text-xs"
        >
          {loading ? 'Resetting...' : 'Reset Password'}
        </button>
        <button
          type="button"
          onClick={onCancel}
          className="btn-secondary flex-1 py-1.5 text-xs"
        >
          Cancel
        </button>
      </div>
    </form>
  );
}

// --- Create User Form ---

function CreateUserForm({
  roleOptions,
  onSubmit,
  onCancel,
}: {
  roleOptions: string[];
  onSubmit: (req: CreateUserRequest) => Promise<void>;
  onCancel: () => void;
}) {
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [role, setRole] = useState<string>('read_only');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (password.length < 4) {
      setError('Password must be at least 4 characters');
      return;
    }
    setLoading(true);
    setError(null);
    try {
      await onSubmit({ username, password, role });
    } catch (err) {
      if (err instanceof ApiClientError) {
        setError(err.message);
      } else {
        setError('Failed to create user');
      }
    } finally {
      setLoading(false);
    }
  };

  return (
    <form
      onSubmit={handleSubmit}
      className="bg-theme-bg-secondary rounded-lg border border-theme-border p-3 space-y-2"
    >
      <h3 className="text-xs font-medium text-theme-text-primary">New User</h3>
      {error && (
        <div className="flex items-center gap-2 p-2 rounded bg-theme-error/10 text-theme-error text-xs">
          <AlertCircle className="w-3 h-3 flex-shrink-0" />
          {error}
        </div>
      )}
      <input
        type="text"
        value={username}
        onChange={(e) => setUsername(e.target.value)}
        placeholder="Username"
        autoComplete="off"
        required
        className="input py-1.5"
      />
      <input
        type="password"
        value={password}
        onChange={(e) => setPassword(e.target.value)}
        placeholder="Password"
        autoComplete="new-password"
        required
        className="input py-1.5"
      />
      <select
        value={role}
        onChange={(e) => setRole(e.target.value)}
        className="select w-full py-1.5"
      >
        {roleOptions.map((r) => (
          <option key={r} value={r}>
            {ROLE_LABELS[r]}
          </option>
        ))}
      </select>
      <div className="flex gap-2">
        <button
          type="submit"
          disabled={loading || !username || !password}
          className="btn-primary flex-1 py-1.5 text-xs"
        >
          {loading ? 'Creating...' : 'Create'}
        </button>
        <button
          type="button"
          onClick={onCancel}
          className="btn-secondary flex-1 py-1.5 text-xs"
        >
          Cancel
        </button>
      </div>
    </form>
  );
}

// --- User Management Modal Wrapper ---

export function UserManagementModal({ isOpen, onClose }: { isOpen: boolean; onClose: () => void }) {
  if (!isOpen) return null;
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      <div className="absolute inset-0 bg-black/60" onClick={onClose} />
      <div className="relative w-full max-w-4xl max-h-[80vh] overflow-y-auto bg-theme-bg-popover rounded-2xl border border-theme-border shadow-2xl p-8 mx-4">
        <div className="flex items-center justify-between mb-4">
          <h2 className="text-lg font-semibold text-theme-text-primary">User Management</h2>
          <button
            className="btn-icon p-2.5 sm:p-1 min-w-[44px] min-h-[44px] sm:min-w-0 sm:min-h-0 flex items-center justify-center"
            onClick={onClose}
          >
            <X className="w-5 h-5" />
          </button>
        </div>
        <UserManagement />
      </div>
    </div>
  );
}
