/**
 * WhitelistManager Modal
 *
 * Full-screen modal for viewing and editing the AT command whitelist.
 * Shows three tabs (Safe / Confirmation / Blocked) with search,
 * source badges, tier move actions, and custom command entry.
 *
 * Search queries across all three tiers simultaneously, showing
 * tier badges inline on each result.
 *
 * Requires Admin+ role with "at-whitelist" feature permission.
 */

import { useState, useEffect, useCallback, useRef, type KeyboardEvent } from 'react';
import { createPortal } from 'react-dom';
import {
  X, Search, Plus, MoreHorizontal, Trash2, Loader2,
  ShieldCheck, ShieldAlert, ShieldBan, Save, RotateCcw,
} from 'lucide-react';
import { getWhitelist, updateWhitelist } from '@/api/modem';
import { useActiveProfile } from '@/hooks/queries/useModemProfiles';
import { useActiveModemId } from '@/hooks/queries/useActiveModemId';
import type { CommandTier, WhitelistEntry, WhitelistOverrides, MergedWhitelist } from '@/types/api';

interface WhitelistManagerProps {
  isOpen: boolean;
  onClose: () => void;
}

const TABS: { tier: CommandTier; label: string; icon: React.ElementType; color: string; activeColor: string }[] = [
  { tier: 'safe', label: 'Safe', icon: ShieldCheck, color: 'text-theme-success', activeColor: 'bg-theme-success/20 border-theme-success/50 text-theme-success' },
  { tier: 'confirmation', label: 'Confirmation', icon: ShieldAlert, color: 'text-theme-warning', activeColor: 'bg-theme-warning/20 border-theme-warning/50 text-theme-warning' },
  { tier: 'blocked', label: 'Blocked', icon: ShieldBan, color: 'text-theme-error', activeColor: 'bg-theme-error/20 border-theme-error/50 text-theme-error' },
];

/** Badge color class by source type. */
const SOURCE_COLOR: Record<string, string> = {
  base: 'bg-theme-bg-tertiary text-theme-text-muted border-theme-border',
  profile: 'bg-theme-accent/20 text-theme-accent border-theme-accent/30',
  custom: 'bg-theme-text-accent/20 text-theme-text-accent border-theme-text-accent/30',
};

/** Tier pill colors for cross-tab search results. */
const TIER_PILL: Record<CommandTier, { label: string; className: string }> = {
  safe: { label: 'Safe', className: 'bg-theme-success/20 text-theme-success border-theme-success/30' },
  confirmation: { label: 'Confirm', className: 'bg-theme-warning/20 text-theme-warning border-theme-warning/30' },
  blocked: { label: 'Blocked', className: 'bg-theme-error/20 text-theme-error border-theme-error/30' },
};

export function WhitelistManager({ isOpen, onClose }: WhitelistManagerProps) {
  const [data, setData] = useState<MergedWhitelist | null>(null);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [activeTab, setActiveTab] = useState<CommandTier>('safe');
  const [search, setSearch] = useState('');
  const [newCommand, setNewCommand] = useState('');
  const [newTier, setNewTier] = useState<CommandTier>('safe');
  const [openMenu, setOpenMenu] = useState<string | null>(null);
  const [dirty, setDirty] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  // Fixed-position menu state
  const [menuPos, setMenuPos] = useState<{ top: number; left: number; openUp: boolean } | null>(null);

  // Active modem info for header + refresh on switch
  const { data: activeModem } = useActiveProfile();
  const modemId = useActiveModemId();
  const modemModel = activeModem?.profile.model ?? 'Unknown Modem';
  const modemProfileId = activeModem?.profile.profile_id;

  // Local working copy of overrides
  const [localOverrides, setLocalOverrides] = useState<WhitelistOverrides>({
    safe_commands: [],
    confirmation_commands: [],
    blocked_prefixes: [],
    tier_overrides: {},
  });

  // Local working copy of commands (rebuilt from data + local changes)
  const [localCommands, setLocalCommands] = useState<WhitelistEntry[]>([]);

  const fetchWhitelist = useCallback(async () => {
    if (!modemId) return;
    setLoading(true);
    setError(null);
    try {
      const result = await getWhitelist(modemId);
      setData(result);
      setLocalOverrides({ ...result.overrides });
      setLocalCommands([...result.commands]);
      setDirty(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load whitelist');
    } finally {
      setLoading(false);
    }
  }, [modemId]);

  // Fetch on open and when modem profile changes
  useEffect(() => {
    if (isOpen) {
      fetchWhitelist();
      setSearch('');
      setNewCommand('');
      setOpenMenu(null);
      setMenuPos(null);
    }
  }, [isOpen, modemProfileId, fetchWhitelist]);

  // Close overflow menu on outside click
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setOpenMenu(null);
        setMenuPos(null);
      }
    };
    if (openMenu) {
      document.addEventListener('mousedown', handler);
      return () => document.removeEventListener('mousedown', handler);
    }
  }, [openMenu]);

  // Close on Escape
  useEffect(() => {
    const handler = (e: globalThis.KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    if (isOpen) {
      document.addEventListener('keydown', handler);
      return () => document.removeEventListener('keydown', handler);
    }
  }, [isOpen, onClose]);

  if (!isOpen) return null;

  // --- Helpers ---

  const applyOverrideLocally = (newOverrides: WhitelistOverrides) => {
    setLocalOverrides(newOverrides);
    setDirty(true);

    // Rebuild commands from the base data + new overrides
    if (!data) return;
    const rebuilt = data.commands.map((cmd) => {
      const overrideTier = newOverrides.tier_overrides[cmd.command.toUpperCase()];
      if (overrideTier) {
        return { ...cmd, tier: overrideTier, overridden: true };
      }
      return { ...cmd, overridden: false };
    });

    // Add custom commands not already in data
    const existingUpper = new Set(rebuilt.map((c) => c.command.toUpperCase()));
    for (const cmd of newOverrides.safe_commands) {
      if (!existingUpper.has(cmd.toUpperCase())) {
        rebuilt.push({ command: cmd, tier: 'safe', source: 'custom', source_label: 'Custom', overridden: false });
        existingUpper.add(cmd.toUpperCase());
      }
    }
    for (const cmd of newOverrides.confirmation_commands) {
      if (!existingUpper.has(cmd.toUpperCase())) {
        rebuilt.push({ command: cmd, tier: 'confirmation', source: 'custom', source_label: 'Custom', overridden: false });
        existingUpper.add(cmd.toUpperCase());
      }
    }
    for (const cmd of newOverrides.blocked_prefixes) {
      if (!existingUpper.has(cmd.toUpperCase())) {
        rebuilt.push({ command: cmd, tier: 'blocked', source: 'custom', source_label: 'Custom', overridden: false });
        existingUpper.add(cmd.toUpperCase());
      }
    }

    setLocalCommands(rebuilt);
  };

  const moveTo = (command: string, source: string, toTier: CommandTier) => {
    const upper = command.toUpperCase();
    const next = { ...localOverrides };

    if (source === 'custom') {
      // Remove from old custom list
      next.safe_commands = next.safe_commands.filter((c) => c.toUpperCase() !== upper);
      next.confirmation_commands = next.confirmation_commands.filter((c) => c.toUpperCase() !== upper);
      next.blocked_prefixes = next.blocked_prefixes.filter((c) => c.toUpperCase() !== upper);
      // Add to new custom list
      if (toTier === 'safe') next.safe_commands.push(command);
      else if (toTier === 'confirmation') next.confirmation_commands.push(command);
      else next.blocked_prefixes.push(command);
      // Remove tier override since it's a custom command
      delete next.tier_overrides[upper];
    } else {
      // Base or profile command — use tier override
      next.tier_overrides = { ...next.tier_overrides, [upper]: toTier };
    }

    applyOverrideLocally(next);
    setOpenMenu(null);
    setMenuPos(null);
  };

  const removeCustom = (command: string) => {
    const upper = command.toUpperCase();
    const next = { ...localOverrides };
    next.safe_commands = next.safe_commands.filter((c) => c.toUpperCase() !== upper);
    next.confirmation_commands = next.confirmation_commands.filter((c) => c.toUpperCase() !== upper);
    next.blocked_prefixes = next.blocked_prefixes.filter((c) => c.toUpperCase() !== upper);
    delete next.tier_overrides[upper];

    applyOverrideLocally(next);
    setLocalCommands((prev) => prev.filter((c) => !(c.source === 'custom' && c.command.toUpperCase() === upper)));
    setOpenMenu(null);
    setMenuPos(null);
  };

  const resetOverride = (command: string) => {
    const upper = command.toUpperCase();
    const next = { ...localOverrides };
    delete next.tier_overrides[upper];
    applyOverrideLocally(next);
    setOpenMenu(null);
    setMenuPos(null);
  };

  const addCommand = () => {
    const trimmed = newCommand.trim().toUpperCase();
    if (!trimmed) return;

    // Check if already exists
    if (localCommands.some((c) => c.command.toUpperCase() === trimmed)) {
      return;
    }

    const next = { ...localOverrides };
    if (newTier === 'safe') next.safe_commands = [...next.safe_commands, trimmed];
    else if (newTier === 'confirmation') next.confirmation_commands = [...next.confirmation_commands, trimmed];
    else next.blocked_prefixes = [...next.blocked_prefixes, trimmed];

    applyOverrideLocally(next);
    setNewCommand('');
  };

  const handleSave = async () => {
    if (!modemId) return;
    setSaving(true);
    setError(null);
    try {
      const result = await updateWhitelist(modemId, localOverrides);
      setData(result);
      setLocalOverrides({ ...result.overrides });
      setLocalCommands([...result.commands]);
      setDirty(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to save whitelist');
    } finally {
      setSaving(false);
    }
  };

  const handleReset = () => {
    if (data) {
      setLocalOverrides({ ...data.overrides });
      setLocalCommands([...data.commands]);
      setDirty(false);
    }
  };

  // Toggle menu and compute fixed position from the button element
  const handleToggleMenu = (command: string, buttonEl: HTMLButtonElement) => {
    if (openMenu === command) {
      setOpenMenu(null);
      setMenuPos(null);
    } else {
      const rect = buttonEl.getBoundingClientRect();
      const spaceBelow = window.innerHeight - rect.bottom;
      const openUp = spaceBelow < 140;
      setOpenMenu(command);
      setMenuPos({
        top: openUp ? rect.top : rect.bottom + 4,
        left: Math.min(rect.right - 160, window.innerWidth - 170),
        openUp,
      });
    }
  };

  // Search mode: when search is non-empty, show all tiers
  const isSearching = search.trim().length > 0;

  // Filter commands for current tab + search
  const filtered = localCommands
    .filter((c) => isSearching || c.tier === activeTab)
    .filter((c) => {
      if (!search) return true;
      const q = search.toLowerCase();
      return c.command.toLowerCase().includes(q);
    })
    .sort((a, b) => {
      // When searching, group by tier first
      if (isSearching) {
        const tierOrd = (t: CommandTier) => t === 'safe' ? 0 : t === 'confirmation' ? 1 : 2;
        const ord = tierOrd(a.tier) - tierOrd(b.tier);
        if (ord !== 0) return ord;
      }
      return a.command.localeCompare(b.command);
    });

  const counts: Record<CommandTier, number> = {
    safe: localCommands.filter((c) => c.tier === 'safe').length,
    confirmation: localCommands.filter((c) => c.tier === 'confirmation').length,
    blocked: localCommands.filter((c) => c.tier === 'blocked').length,
  };

  const handleAddKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      addCommand();
    }
  };

  // Find the entry for the currently open menu (for rendering the floating menu)
  const openEntry = openMenu ? localCommands.find((c) => c.command === openMenu) : null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50" onClick={onClose}>
      <div
        className="bg-theme-bg-card border border-theme-border rounded-xl shadow-xl w-full max-w-xl mx-4 max-h-[85vh] flex flex-col"
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-5 py-4 border-b border-theme-border shrink-0">
          <div>
            <h2 className="text-sm font-medium text-theme-text-primary">AT Command Whitelist</h2>
            <p className="text-xs text-theme-text-muted mt-0.5">
              {modemModel} — Manage allowed, confirmation, and blocked commands
            </p>
          </div>
          <button
            onClick={onClose}
            className="btn-icon p-1.5"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        {/* Loading / Error */}
        {loading ? (
          <div className="flex-1 flex items-center justify-center py-12">
            <Loader2 className="w-5 h-5 text-theme-text-muted animate-spin" />
            <span className="ml-2 text-sm text-theme-text-muted">Loading whitelist...</span>
          </div>
        ) : error && !data ? (
          <div className="flex-1 flex items-center justify-center py-12 px-5">
            <div className="text-center">
              <p className="text-sm text-theme-error mb-2">{error}</p>
              <button
                onClick={fetchWhitelist}
                className="text-xs text-theme-accent hover:underline"
              >
                Retry
              </button>
            </div>
          </div>
        ) : (
          <>
            {/* Tabs */}
            <div className="flex gap-2 px-5 pt-4 pb-2 shrink-0">
              {TABS.map((tab) => {
                const isActive = !isSearching && activeTab === tab.tier;
                const TabIcon = tab.icon;
                return (
                  <button
                    key={tab.tier}
                    onClick={() => { setActiveTab(tab.tier); setSearch(''); }}
                    className={`flex items-center gap-1.5 px-3 py-1.5 rounded-lg border text-xs font-medium transition-colors ${
                      isActive
                        ? tab.activeColor
                        : 'border-theme-border text-theme-text-muted hover:text-theme-text-secondary hover:border-theme-border'
                    }`}
                  >
                    <TabIcon className="w-3.5 h-3.5" />
                    {tab.label}
                    <span className={`ml-1 text-[10px] px-1.5 rounded-full ${
                      isActive ? 'bg-white/10' : 'bg-theme-bg-tertiary'
                    }`}>
                      {counts[tab.tier]}
                    </span>
                  </button>
                );
              })}
            </div>

            {/* Search */}
            <div className="px-5 pb-2 shrink-0">
              <div className="relative">
                <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-theme-text-muted" />
                <input
                  type="text"
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                  placeholder="Search commands across all tiers..."
                  className="input pl-8 pr-3 py-1.5"
                />
              </div>
            </div>

            {/* Error banner (if we have data but save failed) */}
            {error && (
              <div className="mx-5 mb-2 px-3 py-2 rounded-lg bg-theme-error/10 border border-theme-error/20 text-theme-error text-xs shrink-0">
                {error}
              </div>
            )}

            {/* Command List */}
            <div className="flex-1 min-h-0 overflow-y-auto px-5 pb-2">
              {filtered.length === 0 ? (
                <div className="py-8 text-center text-sm text-theme-text-muted">
                  {search ? 'No commands match your search.' : 'No commands in this tier.'}
                </div>
              ) : (
                <div className="space-y-1">
                  {filtered.map((entry) => (
                    <CommandRow
                      key={entry.command}
                      entry={entry}
                      isMenuOpen={openMenu === entry.command}
                      showTierBadge={isSearching}
                      onToggleMenu={(btnEl) => handleToggleMenu(entry.command, btnEl)}
                    />
                  ))}
                </div>
              )}
            </div>

            {/* Add Command */}
            <div className="px-5 py-3 border-t border-theme-border shrink-0">
              <div className="flex gap-2">
                <input
                  type="text"
                  value={newCommand}
                  onChange={(e) => setNewCommand(e.target.value)}
                  onKeyDown={handleAddKeyDown}
                  placeholder="Add AT command (e.g. AT+EXAMPLE)"
                  className="input flex-1 py-1.5 font-mono"
                />
                <select
                  value={newTier}
                  onChange={(e) => setNewTier(e.target.value as CommandTier)}
                  className="select py-1.5 text-xs"
                >
                  <option value="safe">Safe</option>
                  <option value="confirmation">Confirmation</option>
                  <option value="blocked">Blocked</option>
                </select>
                <button
                  onClick={addCommand}
                  disabled={!newCommand.trim()}
                  className="btn-primary px-3 py-1.5 text-xs"
                >
                  <Plus className="w-3.5 h-3.5" />
                </button>
              </div>
            </div>

            {/* Footer */}
            <div className="flex items-center justify-between px-5 py-3 border-t border-theme-border shrink-0">
              <div className="text-xs text-theme-text-muted">
                {dirty ? 'Unsaved changes' : 'No changes'}
              </div>
              <div className="flex gap-2">
                <button
                  onClick={handleReset}
                  disabled={!dirty || saving}
                  className="btn-secondary flex items-center gap-1.5 px-3 py-1.5 text-xs"
                >
                  <RotateCcw className="w-3 h-3" />
                  Reset
                </button>
                <button
                  onClick={handleSave}
                  disabled={!dirty || saving}
                  className="btn-primary flex items-center gap-1.5 px-3 py-1.5 text-xs"
                >
                  {saving ? (
                    <Loader2 className="w-3 h-3 animate-spin" />
                  ) : (
                    <Save className="w-3 h-3" />
                  )}
                  Save
                </button>
              </div>
            </div>
          </>
        )}
      </div>

      {/* Floating overflow menu — rendered via portal to avoid scroll clipping */}
      {openMenu && openEntry && menuPos && createPortal(
        <div
          ref={menuRef}
          style={{
            position: 'fixed',
            top: menuPos.openUp ? undefined : menuPos.top,
            bottom: menuPos.openUp ? (window.innerHeight - menuPos.top + 4) : undefined,
            left: menuPos.left,
          }}
          className="bg-theme-bg-card border border-theme-border rounded-lg shadow-lg py-1 z-[60] min-w-[160px]"
        >
          {TABS.filter((t) => t.tier !== openEntry.tier).map((tab) => {
            const Icon = tab.icon;
            return (
              <button
                key={tab.tier}
                onClick={() => moveTo(openEntry.command, openEntry.source, tab.tier)}
                className="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-theme-text-secondary hover:bg-theme-bg-tertiary hover:text-theme-text-primary transition-colors"
              >
                <Icon className={`w-3 h-3 ${tab.color}`} />
                Move to {tab.label}
              </button>
            );
          })}
          {openEntry.overridden && (
            <button
              onClick={() => resetOverride(openEntry.command)}
              className="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-theme-text-secondary hover:bg-theme-bg-tertiary hover:text-theme-text-primary transition-colors border-t border-theme-border"
            >
              <RotateCcw className="w-3 h-3 text-theme-text-muted" />
              Reset to default
            </button>
          )}
          {openEntry.source === 'custom' && (
            <button
              onClick={() => removeCustom(openEntry.command)}
              className="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-theme-error hover:bg-theme-error/10 transition-colors border-t border-theme-border"
            >
              <Trash2 className="w-3 h-3" />
              Remove
            </button>
          )}
        </div>,
        document.body
      )}
    </div>
  );
}

// --- Command Row ---

interface CommandRowProps {
  entry: WhitelistEntry;
  isMenuOpen: boolean;
  showTierBadge: boolean;
  onToggleMenu: (buttonEl: HTMLButtonElement) => void;
}

function CommandRow({ entry, isMenuOpen, showTierBadge, onToggleMenu }: CommandRowProps) {
  const buttonRef = useRef<HTMLButtonElement>(null);
  const badgeColor = SOURCE_COLOR[entry.source] ?? SOURCE_COLOR['base'];
  const tierPill = TIER_PILL[entry.tier];

  return (
    <div className="flex items-center gap-2 px-3 py-1.5 rounded-lg bg-theme-bg-secondary hover:bg-theme-bg-tertiary transition-colors group">
      {/* Command */}
      <span className="flex-1 font-mono text-xs text-theme-text-primary truncate">
        {entry.command}
      </span>

      {/* Tier badge (shown during cross-tab search) */}
      {showTierBadge && (
        <span className={`text-[10px] px-1.5 py-0.5 rounded border shrink-0 ${tierPill.className}`}>
          {tierPill.label}
        </span>
      )}

      {/* Source label badge */}
      <span className={`text-[10px] px-1.5 py-0.5 rounded border ${badgeColor} shrink-0`}>
        {entry.source_label}
      </span>

      {/* Override indicator */}
      {entry.overridden && (
        <span className="text-[10px] px-1.5 py-0.5 rounded border border-theme-warning/30 bg-theme-warning/10 text-theme-warning shrink-0">
          moved
        </span>
      )}

      {/* Actions menu trigger */}
      <div className="shrink-0">
        <button
          ref={buttonRef}
          onClick={() => { if (buttonRef.current) onToggleMenu(buttonRef.current); }}
          className={`p-1 rounded text-theme-text-muted hover:text-theme-text-primary hover:bg-theme-bg-primary transition-opacity ${
            isMenuOpen ? 'opacity-100' : 'opacity-0 group-hover:opacity-100'
          }`}
        >
          <MoreHorizontal className="w-3.5 h-3.5" />
        </button>
      </div>
    </div>
  );
}
