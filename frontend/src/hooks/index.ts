// Query hooks (GET requests)
export {
  useModemStatus,
  modemStatusQueryKey,
  useSignal,
  signalQueryKey,
  useSimStatus,
  simStatusQueryKey,
  useModemDetection,
  modemDetectionQueryKey,
  useDeviceInfo,
  deviceInfoQueryKey,
  useConfig,
  configQueryKey,
  useVersion,
  versionQueryKey,
  useUpdateCheck,
  updateCheckQueryKey,
  useUpdateStatus,
  updateStatusQueryKey,
  useModemProfiles,
  modemProfilesQueryKey,
  useActiveProfile,
  activeProfileQueryKey,
  useDetectedModems,
  detectedModemsQueryKey,
  useSelectModem,
  useOverrideProfile,
  useRequestProfile,
  useGps,
  gpsQueryKey,
  useExtendedSignal,
  extendedSignalQueryKey,
  useAntennaMetrics,
  antennaMetricsQueryKey,
  useBandConfig,
  bandConfigQueryKey,
  useApnProfiles,
  apnProfilesQueryKey,
  useSimSlots,
  simSlotsQueryKey,
  useWanStatus,
  wanStatusQueryKey,
  useSignalHistory,
  signalHistoryQueryKey,
  useActiveModemId,
  useSpeedtestHistory,
  useRunSpeedtest,
  speedtestHistoryQueryKey,
  usePdpDetails,
  pdpDetailsQueryKey,
} from './queries';

// Mutation hooks (POST/PUT requests)
export {
  useConnect,
  useDisconnect,
  useATCommand,
  usePinOperation,
  useUpdateConfig,
  useNetworkScan,
  useNetworkSelect,
  useApplyUpdate,
  useSetBandConfig,
  useRestoreBands,
  useSelectMbnProfile,
  useDeactivateMbnProfile,
  useSetMbnAutoSelect,
  useCreateApnProfile,
  useUpdateApnProfile,
  useDeleteApnProfile,
  useApplyApnProfile,
  useImportApnProfiles,
  useUpdateSimSlotConfig,
  useSwitchSimSlot,
  useApplyWanConfig,
  useScanWanModems,
  useApplyApn,
  useReconnect,
} from './mutations';

// WebSocket hook
export { useWebSocket, type ConnectionStatus } from './useWebSocket';

// Theme hook
export { useTheme } from './useTheme';

// Auth hook
export { useAuth, type AuthState, type UserInfo } from './useAuth';

// Preset sync hook
export { usePresetSync } from './usePresetSync';

// Page visibility hook
export { usePageVisibility } from './usePageVisibility';
