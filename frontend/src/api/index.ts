// API Client
export { apiGet, apiPost, apiPut, apiDelete, createEventSocket, ApiClientError } from './client';

// Auth API
export { getAuthStatus, login, logout, setupPassword, changePassword } from './auth';
export type { AuthStatus, AuthResult, LoginResult } from './auth';

// Users API
export { listUsers, createUser, updateUser, deleteUser, resetUserPassword } from './users';
export type { UserInfo as ApiUserInfo, CreateUserRequest, UpdateUserRequest } from './users';

// Modem API
export {
  detectModems,
  getModemStatus,
  getSignalInfo,
  getGpsInfo,
  getExtendedSignalInfo,
  getAntennaMetrics,
  getPdpDetails,
  connect,
  disconnect,
  executeATCommand,
  getBandConfig,
  setBandConfig,
  restoreBands,
  selectMbnProfile,
  deactivateMbnProfile,
  setMbnAutoSelect,
  getApnProfiles,
  createApnProfile,
  updateApnProfile,
  deleteApnProfile,
  applyApnProfile,
  exportApnProfiles,
  importApnProfiles,
  getSignalHistory,
  applyApn,
  reconnect,
} from './modem';
export type { PdpDetails } from './modem';

// SIM API
export { getSimStatus, pinOperation, getSimSlots, getSimSlotConfig, updateSimSlotConfig, switchSimSlot } from './sim';

// Network & Config API
export { scanNetworks, selectNetwork, getConfig, updateConfig } from './config';

// System API
export { getVersion, checkForUpdate, applyUpdate, getUpdateStatus, getUpdateLog } from './system';
export type { VersionInfo, UpdateCheckResult, UpdateApplyResult, UpdateStatus } from './system';

// WAN Manager API
export {
  getWanStatus, updateWanConfig, scanWanModems,
  getWatchdogLog, clearWatchdogLog, downloadWatchdogLog,
} from './wan';

// Speedtest API
export { runSpeedtest, getSpeedtestStatus, getSpeedtestHistory } from './speedtest';

// Modem Profiles API
export {
  getModemProfiles,
  getActiveProfile,
  getDetectedModems,
  selectModem,
  overrideProfile,
  requestProfile,
} from './profiles';
