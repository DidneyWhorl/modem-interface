/**
 * View Preset Bar
 *
 * Renders clickable preset icons in the header. Users can switch views
 * with a single click, create new presets, and edit/delete existing ones.
 */

import { useState, useEffect, useRef, useCallback } from 'react';
import { Plus, X, Trash2, Check, Info } from 'lucide-react';
import clsx from 'clsx';
import { useUIStore } from '@/stores/uiStore';
import { IconPicker, PresetIcon } from './IconPicker';
import { MAX_PRESETS, MAX_PRESET_NAME_LENGTH, DEFAULT_PRESET_ICON } from '@/types/presets';

// === Main Component ===

export function ViewPresetBar() {
  const presets = useUIStore((s) => s.presets);
  const activePresetId = useUIStore((s) => s.activePresetId);
  const switchPreset = useUIStore((s) => s.switchPreset);
  const createPreset = useUIStore((s) => s.createPreset);

  const [editingId, setEditingId] = useState<string | null>(null);
  const [showCreate, setShowCreate] = useState(false);
  const initedRef = useRef(false);

  // Auto-create default preset on first render if none exist
  useEffect(() => {
    if (initedRef.current) return;
    initedRef.current = true;
    if (presets.length === 0) {
      createPreset('Default', DEFAULT_PRESET_ICON);
    }
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const [showTooltip, setShowTooltip] = useState(false);

  return (
    <div className="flex items-center gap-2 flex-wrap">
      {/* Label */}
      <div className="relative flex items-center gap-1 shrink-0">
        <span className="text-[10px] font-bold uppercase tracking-wider text-theme-text-muted">
          Dashboards
        </span>
        <button
          type="button"
          className="text-theme-text-muted hover:text-theme-text-secondary transition-colors"
          onMouseEnter={() => setShowTooltip(true)}
          onMouseLeave={() => setShowTooltip(false)}
          onClick={() => setShowTooltip(!showTooltip)}
        >
          <Info className="w-3 h-3" />
        </button>
        {showTooltip && (
          <div className={clsx(
            'absolute top-full left-0 mt-1.5 z-50 w-52 p-2',
            'bg-theme-bg-popover border border-theme-border rounded-lg shadow-lg',
            'text-xs text-theme-text-secondary leading-relaxed'
          )}>
            Saved dashboard layouts. Click to switch, click active to edit. Tap <strong className="text-theme-text-primary">+</strong> to save a new dashboard.
          </div>
        )}
      </div>

      {/* Preset buttons */}
      {presets.map((preset) => (
        <div key={preset.id} className="relative">
          <button
            type="button"
            onClick={() => {
              if (preset.id === activePresetId) {
                setEditingId(editingId === preset.id ? null : preset.id);
              } else {
                setEditingId(null);
                switchPreset(preset.id);
              }
            }}
            className={clsx(
              'px-2.5 py-1.5 rounded-lg text-sm font-medium transition-colors',
              'border select-none',
              preset.id === activePresetId
                ? 'bg-theme-accent/20 text-theme-text-accent border-theme-accent'
                : 'bg-theme-bg-tertiary text-theme-text-secondary border-theme-border hover:bg-theme-bg-secondary hover:text-theme-text-primary'
            )}
            title={preset.name}
          >
            <PresetIcon name={preset.icon} className="w-4 h-4" />
          </button>

          {/* Edit popover for active preset */}
          {editingId === preset.id && (
            <PresetEditPopover
              preset={preset}
              canDelete={presets.length > 1}
              onClose={() => setEditingId(null)}
            />
          )}
        </div>
      ))}

      {/* Create button */}
      {presets.length < MAX_PRESETS && (
        <div className="relative">
          <button
            type="button"
            onClick={() => {
              setEditingId(null);
              setShowCreate(!showCreate);
            }}
            className={clsx(
              'px-2.5 py-1.5 rounded-lg text-sm transition-colors',
              'border border-dashed border-theme-border',
              'text-theme-text-muted hover:text-theme-text-secondary hover:border-theme-text-muted'
            )}
            title="Create new dashboard"
          >
            <Plus className="w-4 h-4" />
          </button>

          {showCreate && (
            <CreatePresetDialog onClose={() => setShowCreate(false)} />
          )}
        </div>
      )}
    </div>
  );
}

// === Edit Popover ===

interface PresetEditPopoverProps {
  preset: { id: string; name: string; icon: string };
  canDelete: boolean;
  onClose: () => void;
}

function PresetEditPopover({ preset, canDelete, onClose }: PresetEditPopoverProps) {
  const updatePresetMeta = useUIStore((s) => s.updatePresetMeta);
  const deletePreset = useUIStore((s) => s.deletePreset);

  const [name, setName] = useState(preset.name);
  const [icon, setIcon] = useState(preset.icon);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const popoverRef = useRef<HTMLDivElement>(null);

  // Click-outside to close
  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (popoverRef.current && !popoverRef.current.contains(e.target as Node)) {
        onClose();
      }
    }
    // Delay listener to avoid immediately closing from the triggering click
    const timer = setTimeout(() => document.addEventListener('mousedown', handleClick), 0);
    return () => {
      clearTimeout(timer);
      document.removeEventListener('mousedown', handleClick);
    };
  }, [onClose]);

  // Escape to close
  useEffect(() => {
    function handleKey(e: KeyboardEvent) {
      if (e.key === 'Escape') onClose();
    }
    document.addEventListener('keydown', handleKey);
    return () => document.removeEventListener('keydown', handleKey);
  }, [onClose]);

  const handleSave = useCallback(() => {
    const trimmed = name.trim() || preset.name;
    updatePresetMeta(preset.id, trimmed, icon);
    onClose();
  }, [name, icon, preset.id, preset.name, updatePresetMeta, onClose]);

  const handleDelete = useCallback(() => {
    if (!confirmDelete) {
      setConfirmDelete(true);
      return;
    }
    deletePreset(preset.id);
    onClose();
  }, [confirmDelete, deletePreset, preset.id, onClose]);

  return (
    <div
      ref={popoverRef}
      className={clsx(
        'absolute top-full left-0 mt-2 z-50',
        'bg-theme-bg-popover border border-theme-border rounded-lg shadow-lg',
        'p-3 w-64'
      )}
    >
      {/* Name input */}
      <label className="block text-xs text-theme-text-secondary mb-1">Name</label>
      <input
        type="text"
        value={name}
        onChange={(e) => setName(e.target.value.slice(0, MAX_PRESET_NAME_LENGTH))}
        maxLength={MAX_PRESET_NAME_LENGTH}
        className="input py-1.5 px-2"
        onKeyDown={(e) => {
          if (e.key === 'Enter') handleSave();
        }}
        autoFocus
      />
      <div className="text-right text-xs text-theme-text-muted mt-0.5">
        {name.length}/{MAX_PRESET_NAME_LENGTH}
      </div>

      {/* Icon picker */}
      <label className="block text-xs text-theme-text-secondary mb-1 mt-2">Icon</label>
      <IconPicker selected={icon} onSelect={setIcon} />

      {/* Actions */}
      <div className="flex items-center justify-between mt-3 pt-2 border-t border-theme-border">
        {canDelete ? (
          <button
            type="button"
            onClick={handleDelete}
            className={clsx(
              'flex items-center gap-1 px-2 py-1 rounded text-xs transition-colors',
              confirmDelete
                ? 'btn-danger px-2 py-1'
                : 'btn-ghost text-theme-text-muted hover:text-theme-error px-2 py-1'
            )}
          >
            <Trash2 className="w-3 h-3" />
            {confirmDelete ? 'Confirm?' : 'Delete'}
          </button>
        ) : (
          <span />
        )}
        <button
          type="button"
          onClick={handleSave}
          className="btn-primary flex items-center gap-1 px-3 py-1 text-xs"
        >
          <Check className="w-3 h-3" />
          Done
        </button>
      </div>
    </div>
  );
}

// === Create Dialog ===

interface CreatePresetDialogProps {
  onClose: () => void;
}

function CreatePresetDialog({ onClose }: CreatePresetDialogProps) {
  const presets = useUIStore((s) => s.presets);
  const createPreset = useUIStore((s) => s.createPreset);

  const [name, setName] = useState(`View ${presets.length + 1}`);
  const [icon, setIcon] = useState(DEFAULT_PRESET_ICON);
  const dialogRef = useRef<HTMLDivElement>(null);

  // Click-outside to close
  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (dialogRef.current && !dialogRef.current.contains(e.target as Node)) {
        onClose();
      }
    }
    const timer = setTimeout(() => document.addEventListener('mousedown', handleClick), 0);
    return () => {
      clearTimeout(timer);
      document.removeEventListener('mousedown', handleClick);
    };
  }, [onClose]);

  // Escape to close
  useEffect(() => {
    function handleKey(e: KeyboardEvent) {
      if (e.key === 'Escape') onClose();
    }
    document.addEventListener('keydown', handleKey);
    return () => document.removeEventListener('keydown', handleKey);
  }, [onClose]);

  const handleCreate = useCallback(() => {
    const trimmed = name.trim() || `View ${presets.length + 1}`;
    createPreset(trimmed, icon);
    onClose();
  }, [name, icon, presets.length, createPreset, onClose]);

  return (
    <div
      ref={dialogRef}
      className={clsx(
        'absolute top-full right-0 mt-2 z-50',
        'bg-theme-bg-popover border border-theme-border rounded-lg shadow-lg',
        'p-3 w-64'
      )}
    >
      <div className="flex items-center justify-between mb-2">
        <span className="text-sm font-medium text-theme-text-primary">New Dashboard</span>
        <button
          type="button"
          onClick={onClose}
          className="text-theme-text-muted hover:text-theme-text-primary transition-colors"
        >
          <X className="w-4 h-4" />
        </button>
      </div>

      {/* Name input */}
      <label className="block text-xs text-theme-text-secondary mb-1">Name</label>
      <input
        type="text"
        value={name}
        onChange={(e) => setName(e.target.value.slice(0, MAX_PRESET_NAME_LENGTH))}
        maxLength={MAX_PRESET_NAME_LENGTH}
        className="input py-1.5 px-2"
        onKeyDown={(e) => {
          if (e.key === 'Enter') handleCreate();
        }}
        autoFocus
      />
      <div className="text-right text-xs text-theme-text-muted mt-0.5">
        {name.length}/{MAX_PRESET_NAME_LENGTH}
      </div>

      {/* Icon picker */}
      <label className="block text-xs text-theme-text-secondary mb-1 mt-2">Icon</label>
      <IconPicker selected={icon} onSelect={setIcon} />

      {/* Actions */}
      <div className="flex items-center justify-end gap-2 mt-3 pt-2 border-t border-theme-border">
        <button
          type="button"
          onClick={onClose}
          className="btn-ghost px-3 py-1 text-xs"
        >
          Cancel
        </button>
        <button
          type="button"
          onClick={handleCreate}
          className="btn-primary flex items-center gap-1 px-3 py-1 text-xs"
        >
          <Plus className="w-3 h-3" />
          Create
        </button>
      </div>
    </div>
  );
}
