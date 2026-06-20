//! SIM card API route handlers.
//!
//! Handlers for /api/sim/* endpoints including status, PIN operations,
//! and dual SIM slot management.

use axum::{
    extract::{Path, State},
    Extension, Json,
};
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

use crate::api::error::{ApiError, ApiResult};
use crate::hardware::{
    DualSimConfig, DualSimInfo, PinOpType, PinOperation, SimSlotConfig, SimSlotStatus,
    SimSlotSwitchRequest, SimSlotSwitchResult, SimStatus,
};
use crate::api::auth_middleware::{require_admin, SessionUser};
use crate::security::audit::AuditEventType;
use crate::state::{debug_trace_with_source, AppState};

/// Helper to get modem handler for this module.
async fn get_modem_handler(
    state: &AppState,
    modem_id: &str,
) -> Result<Arc<tokio::sync::Mutex<Box<dyn crate::hardware::ModemHardware + Send>>>, ApiError> {
    let modems = state.modems.read().await;
    let context = modems.get(modem_id).ok_or_else(|| {
        ApiError::not_found(format!("Modem not found: {modem_id}"))
    })?;
    let handler = Arc::clone(&context.handler);
    drop(modems);
    Ok(handler)
}

/// Get modem's VID:PID key for SIM slot config lookup.
/// Used to identify which modem's SIM slot config to use.
async fn get_modem_vid_pid_key(state: &AppState, modem_id: &str) -> Option<String> {
    let modems = state.modems.read().await;
    let context = modems.get(modem_id)?;
    match (&context.detected.vendor_id, &context.detected.product_id) {
        (Some(vid), Some(pid)) => Some(format!("{vid}:{pid}")),
        _ => None,
    }
}

/// Get modem's dual SIM config from profile.
async fn get_modem_dual_sim_config(state: &AppState, modem_id: &str) -> Option<DualSimConfig> {
    let modems = state.modems.read().await;
    let context = modems.get(modem_id)?;
    Some(context.profile.dual_sim_config.clone())
}


/// Get SIM slot config for the given modem key, migrating "legacy" data if needed.
///
/// After upgrading from the old flat config format, slot assignments live under
/// the `"legacy"` key. On first access for a real modem VID:PID, this helper
/// moves the data under the correct key and persists the change.
async fn get_modem_sim_config(state: &AppState, modem_key: &str) -> SimSlotConfig {
    // Fast path: read lock
    {
        let all_config = state.sim_slot_config.read().await;
        if let Some(cfg) = all_config.modems.get(modem_key) {
            return cfg.clone();
        }
        if !all_config.modems.contains_key("legacy") {
            return SimSlotConfig::default();
        }
    }
    // Slow path: migrate "legacy" → actual modem key
    let mut all_config = state.sim_slot_config.write().await;
    // Re-check after acquiring write lock (another request may have migrated)
    if let Some(cfg) = all_config.modems.get(modem_key) {
        return cfg.clone();
    }
    if let Some(legacy) = all_config.modems.remove("legacy") {
        tracing::info!("Migrating 'legacy' SIM slot config to modem key '{modem_key}'");
        all_config.modems.insert(modem_key.to_string(), legacy);
        let snapshot = all_config.clone();
        let result = all_config.modems.get(modem_key).cloned().unwrap_or_default();
        drop(all_config);
        if let Err(e) = crate::config::sim_slots::save_sim_slot_config(&snapshot).await {
            tracing::warn!("Failed to save migrated SIM slot config: {e}");
        }
        return result;
    }
    SimSlotConfig::default()
}

const QUICK_TIMEOUT: Duration = Duration::from_secs(5);
const STATE_CHANGE_TIMEOUT: Duration = Duration::from_secs(15);

/// GET /api/modem/:modem_id/sim/status
///
/// Get SIM card status from the discovery cache (populated at boot, refreshable via
/// POST /api/modem/:modem_id/sim/refresh).
pub async fn status(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<SimStatus>> {
    let modems = state.modems.read().await;
    let context = modems.get(&modem_id).ok_or_else(|| {
        ApiError::not_found(format!("Modem not found: {modem_id}"))
    })?;
    let discovery = context.discovery.read().await;
    Ok(Json(discovery.sim_status.clone()))
}

/// PIN operation response.
#[derive(Debug, Serialize)]
pub struct PinResponse {
    pub success: bool,
    pub message: String,
}

/// POST /api/modem/:modem_id/sim/pin
///
/// Perform PIN operations: verify, change, enable, disable.
pub async fn pin(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(op): Json<PinOperation>,
) -> ApiResult<Json<PinResponse>> {
    require_admin(&session_user)?;

    // Validate PIN format
    if op.pin.len() < 4 || op.pin.len() > 8 {
        return Err(ApiError::bad_request("PIN must be 4-8 digits"));
    }
    if !op.pin.chars().all(|c| c.is_ascii_digit()) {
        return Err(ApiError::bad_request("PIN must contain only digits"));
    }

    let handler_arc = get_modem_handler(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    match op.operation {
        PinOpType::Verify => {
            timeout(STATE_CHANGE_TIMEOUT, modem.verify_pin(&op.pin))
                .await
                .map_err(|_| ApiError::service_unavailable_with_retry("PIN verification timed out", 5))?
                .map_err(ApiError::from)?;

            Ok(Json(PinResponse {
                success: true,
                message: "PIN verified successfully".to_string(),
            }))
        }
        PinOpType::Change => {
            let new_pin = op.new_pin.as_ref().ok_or_else(|| {
                ApiError::bad_request("new_pin required for change operation")
            })?;

            if new_pin.len() < 4 || new_pin.len() > 8 {
                return Err(ApiError::bad_request("New PIN must be 4-8 digits"));
            }
            if !new_pin.chars().all(|c| c.is_ascii_digit()) {
                return Err(ApiError::bad_request("New PIN must contain only digits"));
            }

            timeout(STATE_CHANGE_TIMEOUT, modem.change_pin(&op.pin, new_pin))
                .await
                .map_err(|_| ApiError::service_unavailable_with_retry("PIN change timed out", 5))?
                .map_err(ApiError::from)?;

            Ok(Json(PinResponse {
                success: true,
                message: "PIN changed successfully".to_string(),
            }))
        }
        PinOpType::Enable => {
            timeout(STATE_CHANGE_TIMEOUT, modem.enable_pin(&op.pin))
                .await
                .map_err(|_| ApiError::service_unavailable_with_retry("Operation timed out", 5))?
                .map_err(ApiError::from)?;

            Ok(Json(PinResponse {
                success: true,
                message: "PIN enabled".to_string(),
            }))
        }
        PinOpType::Disable => {
            timeout(STATE_CHANGE_TIMEOUT, modem.disable_pin(&op.pin))
                .await
                .map_err(|_| ApiError::service_unavailable_with_retry("Operation timed out", 5))?
                .map_err(ApiError::from)?;

            Ok(Json(PinResponse {
                success: true,
                message: "PIN disabled".to_string(),
            }))
        }
    }
}

// =============================================================================
// Dual SIM Slot Management
// =============================================================================

/// GET /api/modem/:modem_id/sim/slots
///
/// Get dual SIM slot information: active slot, per-slot SIM status and assigned profiles.
pub async fn get_sim_slots(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<DualSimInfo>> {
    // Get dual SIM config from modem profile
    let dsim = match get_modem_dual_sim_config(&state, &modem_id).await {
        Some(config) if config.supported => config,
        _ => {
            return Ok(Json(DualSimInfo {
                supported: false,
                dual_sim_disabled: false,
                slot_count: 1,
                active_slot: 1,
                slots: vec![],
            }));
        }
    };

    // Check per-modem config (migrates "legacy" key if needed)
    let modem_key = get_modem_vid_pid_key(&state, &modem_id).await.unwrap_or_else(|| modem_id.clone());
    let slot_config = get_modem_sim_config(&state, &modem_key).await;
    if slot_config.dual_sim_disabled {
        return Ok(Json(DualSimInfo {
            supported: false,
            dual_sim_disabled: true,
            slot_count: dsim.slot_count,
            active_slot: 1,
            slots: vec![],
        }));
    }

    // Query active slot from modem
    let handler_arc = get_modem_handler(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;
    let active_slot = if let Some(ref cmd) = dsim.query_slot_cmd {
        match timeout(QUICK_TIMEOUT, modem.execute_at(cmd)).await {
            Ok(Ok(resp)) => parse_slot_number(&resp, dsim.query_slot_regex.as_deref()),
            _ => 1, // Default to slot 1 on error
        }
    } else {
        1
    };

    // Get SIM status for the active slot
    let sim_status = match timeout(QUICK_TIMEOUT, modem.get_sim_status()).await {
        Ok(Ok(s)) => Some(s),
        _ => None,
    };
    drop(modem);

    // slot_config already fetched above (with legacy migration)
    let apn_profiles = state.apn_profiles.read().await;

    let mut slots = Vec::new();
    for slot_num in 1..=dsim.slot_count {
        let is_active = slot_num == active_slot;
        let assigned_id = match slot_num {
            1 => slot_config.slot1_profile_id.clone(),
            2 => slot_config.slot2_profile_id.clone(),
            _ => None,
        };
        let assigned_name = assigned_id.as_ref().and_then(|id| {
            apn_profiles.iter().find(|p| p.id == *id).map(|p| p.name.clone())
        });

        slots.push(SimSlotStatus {
            slot: slot_num,
            active: is_active,
            sim_status: if is_active { sim_status.clone() } else { None },
            assigned_profile_id: assigned_id,
            assigned_profile_name: assigned_name,
        });
    }

    Ok(Json(DualSimInfo {
        supported: true,
        dual_sim_disabled: false,
        slot_count: dsim.slot_count,
        active_slot,
        slots,
    }))
}

/// GET /api/modem/:modem_id/sim/slots/config
///
/// Get per-slot APN profile assignments for the specified modem.
pub async fn get_sim_slot_config(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<SimSlotConfig>> {
    let key = get_modem_vid_pid_key(&state, &modem_id).await.unwrap_or_else(|| modem_id.clone());
    let config = get_modem_sim_config(&state, &key).await;
    Ok(Json(config))
}

/// PUT /api/modem/:modem_id/sim/slots/config
///
/// Update per-slot APN profile assignments for the specified modem. Persists to disk.
pub async fn update_sim_slot_config(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(req): Json<SimSlotConfig>,
) -> ApiResult<Json<SimSlotConfig>> {
    require_admin(&session_user)?;

    tracing::info!(
        "update_sim_slot_config: entry user={} modem={} slot1={:?} slot2={:?} disabled={}",
        session_user.username,
        modem_id,
        req.slot1_profile_id,
        req.slot2_profile_id,
        req.dual_sim_disabled,
    );

    // Validate that referenced profile IDs exist
    let profiles = state.apn_profiles.read().await;
    if let Some(ref id) = req.slot1_profile_id {
        if !profiles.iter().any(|p| p.id == *id) {
            return Err(ApiError::bad_request("Slot 1 profile ID not found"));
        }
    }
    if let Some(ref id) = req.slot2_profile_id {
        if !profiles.iter().any(|p| p.id == *id) {
            return Err(ApiError::bad_request("Slot 2 profile ID not found"));
        }
    }
    drop(profiles);
    tracing::info!("update_sim_slot_config: profile validation passed");

    let key = get_modem_vid_pid_key(&state, &modem_id).await.unwrap_or_else(|| modem_id.clone());
    tracing::info!("update_sim_slot_config: modem_key={key}");

    // Merge with existing config, then release write lock BEFORE disk I/O
    let (merged, config_snapshot) = {
        tracing::info!("update_sim_slot_config: acquiring sim_slot_config write lock");
        let mut all_config = state.sim_slot_config.write().await;
        tracing::info!("update_sim_slot_config: write lock acquired");

        // Migrate "legacy" key to actual modem key if needed
        if !all_config.modems.contains_key(&key) {
            if let Some(legacy) = all_config.modems.remove("legacy") {
                tracing::info!("Migrating 'legacy' SIM slot config to modem key '{key}'");
                all_config.modems.insert(key.clone(), legacy);
            }
        }

        let existing = all_config.modems.get(&key).cloned().unwrap_or_default();
        let merged = SimSlotConfig {
            slot1_profile_id: req.slot1_profile_id.or(existing.slot1_profile_id),
            slot2_profile_id: req.slot2_profile_id.or(existing.slot2_profile_id),
            dual_sim_disabled: req.dual_sim_disabled,
        };
        all_config.modems.insert(key.clone(), merged.clone());
        let snapshot = all_config.clone();
        (merged, snapshot)
    }; // write lock released here

    // Save to disk OUTSIDE the write lock (flash I/O can be slow)
    tracing::info!("update_sim_slot_config: saving to disk");
    if let Err(e) = crate::config::sim_slots::save_sim_slot_config(&config_snapshot).await {
        tracing::warn!("Failed to save SIM slot config: {e}");
    }
    tracing::info!("update_sim_slot_config: save complete");

    // Audit log
    state.audit.log(
        AuditEventType::ConfigChanged,
        None,
        format!(
            "{} updated SIM slot config for {}: slot1={}, slot2={}, disabled={}",
            session_user.username,
            key,
            merged.slot1_profile_id.as_deref().unwrap_or("none"),
            merged.slot2_profile_id.as_deref().unwrap_or("none"),
            merged.dual_sim_disabled,
        ),
    ).await;

    tracing::info!("update_sim_slot_config: done");
    Ok(Json(merged))
}

/// POST /api/modem/:modem_id/sim/slots/switch
///
/// Switch active SIM slot. Supports simple swap (just AT+QUIMSLOT) or full swap
/// (switch slot + apply assigned APN profile + reboot).
pub async fn switch_sim_slot(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(req): Json<SimSlotSwitchRequest>,
) -> ApiResult<Json<SimSlotSwitchResult>> {
    require_admin(&session_user)?;

    // Get dual SIM config and APN apply config from modem profile
    let (dsim, apply_cfg) = {
        let modems = state.modems.read().await;
        let context = modems.get(&modem_id).ok_or_else(|| {
            ApiError::not_found(format!("Modem not found: {modem_id}"))
        })?;
        let dsim = context.profile.dual_sim_config.clone();
        let apply_cfg = context.profile.apn_apply_config.clone();
        (dsim, apply_cfg)
    };

    if !dsim.supported {
        return Err(ApiError::bad_request("Dual SIM not supported for this modem"));
    }

    // Validate target slot
    if req.target_slot < 1 || req.target_slot > dsim.slot_count {
        return Err(ApiError::bad_request(format!(
            "Invalid slot: must be 1-{}", dsim.slot_count
        )));
    }

    let set_slot_cmd = dsim.set_slot_cmd.as_ref()
        .ok_or_else(|| ApiError::bad_request("SIM slot switch command not configured"))?;

    let mut step_log = Vec::new();

    // Query current slot BEFORE switching (for fallback)
    let handler_arc = get_modem_handler(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;
    let original_slot = if let Some(ref cmd) = dsim.query_slot_cmd {
        match timeout(QUICK_TIMEOUT, modem.execute_at(cmd)).await {
            Ok(Ok(resp)) => parse_slot_number(&resp, dsim.query_slot_regex.as_deref()),
            _ => if req.target_slot == 1 { 2 } else { 1 },
        }
    } else if req.target_slot == 1 { 2 } else { 1 };
    debug_trace_with_source(format!("[SIM-SLOT] Current slot: {original_slot}, target: {}", req.target_slot), "sim");

    // Step 1: Switch SIM slot
    let cmd = set_slot_cmd.replace("{slot}", &req.target_slot.to_string());
    debug_trace_with_source(format!("[SIM-SLOT] Switching to slot {}: {cmd}", req.target_slot), "sim");

    match timeout(STATE_CHANGE_TIMEOUT, modem.execute_at(&cmd)).await {
        Ok(Ok(resp)) if !resp.contains("ERROR") => {
            step_log.push(format!("OK: Switch to slot {}", req.target_slot));
            debug_trace_with_source(format!("[SIM-SLOT] Slot switch OK: {}", resp.trim()), "sim");
        }
        Ok(Ok(resp)) => {
            let msg = format!("Slot switch failed: {}", resp.trim());
            debug_trace_with_source(format!("[SIM-SLOT] {msg}"), "sim");
            return Ok(Json(SimSlotSwitchResult {
                success: false,
                rebooting: false,
                message: msg.clone(),
                steps: vec![format!("FAIL: {msg}")],
            }));
        }
        Ok(Err(e)) => {
            let msg = format!("Slot switch error: {e}");
            debug_trace_with_source(format!("[SIM-SLOT] {msg}"), "sim");
            return Ok(Json(SimSlotSwitchResult {
                success: false,
                rebooting: false,
                message: msg.clone(),
                steps: vec![format!("FAIL: {msg}")],
            }));
        }
        Err(_) => {
            let msg = "Slot switch timed out".to_string();
            debug_trace_with_source(format!("[SIM-SLOT] {msg}"), "sim");
            return Ok(Json(SimSlotSwitchResult {
                success: false,
                rebooting: false,
                message: msg.clone(),
                steps: vec![format!("FAIL: {msg}")],
            }));
        }
    }

    // Step 2: Wait for SIM initialization on the new slot
    let sim_present = wait_for_sim_init(&modem, &dsim, &mut step_log).await;

    // Step 3: If no SIM detected, fallback to original slot
    if !sim_present {
        debug_trace_with_source(format!(
            "[SIM-SLOT] No SIM in slot {} — falling back to slot {original_slot}",
            req.target_slot
        ), "sim");
        step_log.push(format!(
            "FAIL: No SIM detected in slot {} — reverting to slot {original_slot}",
            req.target_slot
        ));

        // Switch back
        let fallback_cmd = set_slot_cmd.replace("{slot}", &original_slot.to_string());
        match timeout(STATE_CHANGE_TIMEOUT, modem.execute_at(&fallback_cmd)).await {
            Ok(Ok(resp)) if !resp.contains("ERROR") => {
                step_log.push(format!("OK: Reverted to slot {original_slot}"));
                debug_trace_with_source(format!("[SIM-SLOT] Fallback to slot {original_slot} OK"), "sim");
            }
            _ => {
                step_log.push(format!("WARN: Fallback to slot {original_slot} failed"));
                debug_trace_with_source(format!("[SIM-SLOT] Fallback to slot {original_slot} FAILED"), "sim");
            }
        }

        // Wait for original SIM to re-init
        let _ = wait_for_sim_init(&modem, &dsim, &mut step_log).await;

        // Re-apply original slot's profile config if it exists
        let modem_key = get_modem_vid_pid_key(&state, &modem_id).await;
        let slot_config = get_modem_sim_config(&state, modem_key.as_deref().unwrap_or("")).await;
        let original_profile_id = match original_slot {
            1 => slot_config.slot1_profile_id.clone(),
            2 => slot_config.slot2_profile_id.clone(),
            _ => None,
        };

        if let Some(pid) = original_profile_id {
            let profiles = state.apn_profiles.read().await;
            if let Some(profile) = profiles.iter().find(|p| p.id == pid) {
                // Restore the original profile's connection config
                let mut app_config = state.config.write().await;
                app_config.connection = profile.connection.clone();
                if let Err(e) = crate::config::save_config(&app_config).await {
                    debug_trace_with_source(format!("[SIM-SLOT] Failed to save restored config: {e}"), "sim");
                }
                step_log.push(format!("OK: Restored profile '{}' config for slot {original_slot}", profile.name));
            }
            drop(profiles);
        }

        drop(modem);

        // Audit log
        state.audit.log(
            AuditEventType::ConfigChanged,
            None,
            format!(
                "{} SIM slot switch to slot {} failed (no SIM), reverted to slot {original_slot}",
                session_user.username, req.target_slot
            ),
        ).await;

        return Ok(Json(SimSlotSwitchResult {
            success: false,
            rebooting: false,
            message: format!(
                "No SIM card in slot {}. Reverted to slot {original_slot}.",
                req.target_slot
            ),
            steps: step_log,
        }));
    }

    // Simple swap: done here
    if !req.apply_profile {
        drop(modem);

        // Audit log
        state.audit.log(
            AuditEventType::ConfigChanged,
            None,
            format!("{} simple SIM slot switch to slot {}", session_user.username, req.target_slot),
        ).await;

        return Ok(Json(SimSlotSwitchResult {
            success: true,
            rebooting: false,
            message: format!("Switched to SIM slot {}", req.target_slot),
            steps: step_log,
        }));
    }

    // Full swap: apply assigned APN profile + reboot
    // Look up the assigned profile for the target slot (per-modem config)
    let modem_key = get_modem_vid_pid_key(&state, &modem_id).await;
    let slot_config = get_modem_sim_config(&state, modem_key.as_deref().unwrap_or("")).await;
    let assigned_profile_id = match req.target_slot {
        1 => slot_config.slot1_profile_id.clone(),
        2 => slot_config.slot2_profile_id.clone(),
        _ => None,
    };

    if let Some(profile_id) = assigned_profile_id {
        let profiles = state.apn_profiles.read().await;
        let apn_profile = profiles.iter().find(|p| p.id == profile_id).cloned();
        drop(profiles);

        if let Some(apn_profile) = apn_profile {
            debug_trace_with_source(format!(
                "[SIM-SLOT] Applying profile '{}' for slot {}",
                apn_profile.name, req.target_slot
            ), "sim");

            let has_mbn = apn_profile.mbn_profile.is_some();
            let ip_type_str = match apn_profile.connection.ip_type {
                crate::hardware::IpType::Ipv4 => "IP",
                crate::hardware::IpType::Ipv6 => "IPV6",
                crate::hardware::IpType::Ipv4v6 => "IPV4V6",
            };

            // Execute apply steps from modem profile
            if apply_cfg.supported {
                for step in &apply_cfg.steps {
                    if step.requires_mbn && !has_mbn {
                        step_log.push(format!("SKIP: {} (no MBN)", step.label));
                        continue;
                    }

                    let at_cmd = step.command
                        .replace("{mbn_profile}", apn_profile.mbn_profile.as_deref().unwrap_or(""))
                        .replace("{cid}", &apn_profile.connection.cid.to_string())
                        .replace("{ip_type}", ip_type_str)
                        .replace("{apn}", &apn_profile.connection.apn);

                    debug_trace_with_source(format!("[SIM-SLOT] Step '{}': {at_cmd}", step.label), "sim");
                    let step_timeout = Duration::from_secs(step.timeout_secs);

                    match timeout(step_timeout, modem.execute_at(&at_cmd)).await {
                        Ok(Ok(resp)) if !resp.contains("ERROR") => {
                            step_log.push(format!("OK: {}", step.label));
                        }
                        Ok(Ok(resp)) => {
                            step_log.push(format!("WARN: {} — {}", step.label, resp.trim()));
                        }
                        Ok(Err(e)) => {
                            step_log.push(format!("WARN: {} — {e}", step.label));
                        }
                        Err(_) => {
                            step_log.push(format!("WARN: {} — timeout", step.label));
                        }
                    }
                }

                // AutoSel fallback for profiles without MBN
                if !has_mbn {
                    let autosel_cmd = r#"AT+QMBNCFG="AutoSel",1"#;
                    debug_trace_with_source(format!("[SIM-SLOT] No MBN — enabling AutoSel: {autosel_cmd}"), "sim");
                    match timeout(STATE_CHANGE_TIMEOUT, modem.execute_at(autosel_cmd)).await {
                        Ok(Ok(resp)) if !resp.contains("ERROR") => {
                            step_log.push("OK: Enable MBN AutoSel (no MBN specified)".into());
                        }
                        _ => {
                            step_log.push("WARN: AutoSel enable failed (continuing)".into());
                        }
                    }
                }
            }

            // Save connection to persistent config
            {
                let mut app_config = state.config.write().await;
                app_config.connection = apn_profile.connection.clone();
                if let Err(e) = crate::config::save_config(&app_config).await {
                    debug_trace_with_source(format!("[SIM-SLOT] Failed to save config: {e}"), "sim");
                }
            }

            step_log.push(format!("OK: Applied profile '{}'", apn_profile.name));
        } else {
            step_log.push("WARN: Assigned profile not found, skipping apply".into());
        }
    } else {
        step_log.push("INFO: No profile assigned to target slot, skipping apply".into());
    }

    // Reboot modem
    if apply_cfg.always_reboot || req.apply_profile {
        if apply_cfg.pre_reboot_delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(apply_cfg.pre_reboot_delay_ms)).await;
        }

        debug_trace_with_source("[SIM-SLOT] Rebooting modem (AT+CFUN=1,1)", "sim");
        match timeout(STATE_CHANGE_TIMEOUT, modem.execute_at("AT+CFUN=1,1")).await {
            Ok(Ok(_)) => {
                step_log.push("OK: Modem reboot initiated".into());
            }
            _ => {
                step_log.push("WARN: Reboot command failed (modem may still restart)".into());
            }
        }
        drop(modem);

        // Set per-modem health to rebooting
        {
            let modems = state.modems.read().await;
            if let Some(context) = modems.get(&modem_id) {
                let mut health = context.health.write().await;
                *health = crate::hardware::ModemHealth {
                    available: false,
                    state: crate::hardware::ModemHealthState::Rebooting,
                    message: Some(format!("SIM slot switch to slot {} — rebooting", req.target_slot)),
                };
            }
        }
        state.broadcast_event(crate::hardware::ModemEvent::ModemHealth(
            crate::hardware::ModemHealth {
                available: false,
                state: crate::hardware::ModemHealthState::Rebooting,
                message: Some("SIM slot switch — rebooting".into()),
            },
        ));

        // Audit log
        state.audit.log(
            AuditEventType::ConfigChanged,
            None,
            format!("{} full SIM slot switch to slot {} with profile apply + reboot", session_user.username, req.target_slot),
        ).await;

        return Ok(Json(SimSlotSwitchResult {
            success: true,
            rebooting: true,
            message: format!("Switched to SIM slot {} — modem rebooting", req.target_slot),
            steps: step_log,
        }));
    }

    drop(modem);

    // Audit log
    state.audit.log(
        AuditEventType::ConfigChanged,
        None,
        format!("{} SIM slot switch to slot {} with profile apply", session_user.username, req.target_slot),
    ).await;

    Ok(Json(SimSlotSwitchResult {
        success: true,
        rebooting: false,
        message: format!("Switched to SIM slot {} and applied profile", req.target_slot),
        steps: step_log,
    }))
}

// =============================================================================
// Helpers
// =============================================================================

/// Wait for SIM initialization after slot switch. Returns true if SIM is present and initialized.
/// Polls AT+QINISTAT (or equivalent) for up to `sim_init_timeout_secs`, then checks AT+CPIN?.
async fn wait_for_sim_init(
    modem: &tokio::sync::MutexGuard<'_, Box<dyn crate::hardware::ModemHardware + Send>>,
    dsim: &DualSimConfig,
    step_log: &mut Vec<String>,
) -> bool {
    // First, wait a moment for the SIM subsystem to start up
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Poll SIM init status if command is configured
    if let Some(ref init_cmd) = dsim.sim_init_cmd {
        debug_trace_with_source("[SIM-SLOT] Waiting for SIM initialization...", "sim");
        let init_timeout = Duration::from_secs(dsim.sim_init_timeout_secs);
        let start = std::time::Instant::now();
        let mut initialized = false;
        let mut saw_not_inserted = false;

        while start.elapsed() < init_timeout {
            // Also check AT+CPIN? for early no-SIM detection
            if let Ok(Ok(cpin_resp)) = timeout(QUICK_TIMEOUT, modem.execute_at("AT+CPIN?")).await {
                let upper = cpin_resp.to_uppercase();
                if upper.contains("NOT INSERTED") || upper.contains("NOT READY") {
                    // NOT READY can be transient during init, but NOT INSERTED is definitive
                    if upper.contains("NOT INSERTED") {
                        saw_not_inserted = true;
                        debug_trace_with_source("[SIM-SLOT] AT+CPIN? reports NOT INSERTED — no SIM in slot", "sim");
                        break;
                    }
                    debug_trace_with_source("[SIM-SLOT] AT+CPIN? reports NOT READY, continuing to poll...", "sim");
                } else if upper.contains("READY") && !upper.contains("NOT") {
                    // SIM is present and ready
                    debug_trace_with_source("[SIM-SLOT] AT+CPIN? reports READY", "sim");
                }
            }

            if let Ok(Ok(resp)) = timeout(QUICK_TIMEOUT, modem.execute_at(init_cmd)).await {
                if let Some(status) = parse_init_status(&resp, dsim.sim_init_regex.as_deref()) {
                    if status >= dsim.sim_init_complete_value {
                        initialized = true;
                        step_log.push(format!("OK: SIM initialized (status={status})"));
                        debug_trace_with_source(format!("[SIM-SLOT] SIM initialized: status={status}"), "sim");
                        break;
                    }
                    debug_trace_with_source(format!("[SIM-SLOT] SIM init status={status}, waiting..."), "sim");
                }
            }

            tokio::time::sleep(Duration::from_secs(2)).await;
        }

        if saw_not_inserted {
            step_log.push("FAIL: No SIM card detected in slot".into());
            return false;
        }

        if !initialized {
            // Final CPIN check after timeout
            if let Ok(Ok(cpin_resp)) = timeout(QUICK_TIMEOUT, modem.execute_at("AT+CPIN?")).await {
                let upper = cpin_resp.to_uppercase();
                if upper.contains("NOT INSERTED") {
                    step_log.push("FAIL: No SIM card detected in slot".into());
                    debug_trace_with_source("[SIM-SLOT] Final CPIN check: NOT INSERTED", "sim");
                    return false;
                }
                if upper.contains("CME ERROR: 10") {
                    // CME ERROR 10 = SIM not inserted
                    step_log.push("FAIL: No SIM card detected in slot (CME ERROR 10)".into());
                    debug_trace_with_source("[SIM-SLOT] Final CPIN check: CME ERROR 10 (no SIM)", "sim");
                    return false;
                }
            }
            step_log.push("WARN: SIM init timeout — may still be initializing".into());
            debug_trace_with_source("[SIM-SLOT] SIM init timeout, continuing anyway", "sim");
        }

        return true;
    }

    // No init command configured — just check CPIN
    if let Ok(Ok(cpin_resp)) = timeout(QUICK_TIMEOUT, modem.execute_at("AT+CPIN?")).await {
        let upper = cpin_resp.to_uppercase();
        if upper.contains("NOT INSERTED") || upper.contains("CME ERROR: 10") {
            step_log.push("FAIL: No SIM card detected in slot".into());
            return false;
        }
    }

    true
}

/// Parse active slot number from AT+QUIMSLOT? response.
fn parse_slot_number(response: &str, regex_pattern: Option<&str>) -> u8 {
    if let Some(pattern) = regex_pattern {
        if let Ok(re) = regex::Regex::new(pattern) {
            if let Some(caps) = re.captures(response) {
                if let Some(m) = caps.get(1) {
                    if let Ok(slot) = m.as_str().parse::<u8>() {
                        return slot;
                    }
                }
            }
        }
    }
    // Fallback: look for a number after "QUIMSLOT:" or "QUSIMSLOT:"
    for line in response.lines() {
        let upper = line.to_uppercase();
        if upper.contains("IMSLOT:") {
            if let Some(num) = line.split(':').next_back() {
                if let Ok(slot) = num.trim().parse::<u8>() {
                    return slot;
                }
            }
        }
    }
    1 // Default to slot 1
}

/// Parse SIM init status bitmask from AT+QINISTAT response.
fn parse_init_status(response: &str, regex_pattern: Option<&str>) -> Option<u8> {
    if let Some(pattern) = regex_pattern {
        if let Ok(re) = regex::Regex::new(pattern) {
            if let Some(caps) = re.captures(response) {
                if let Some(m) = caps.get(1) {
                    return m.as_str().parse::<u8>().ok();
                }
            }
        }
    }
    // Fallback
    for line in response.lines() {
        if line.contains("QINISTAT:") {
            if let Some(num) = line.split(':').next_back() {
                return num.trim().parse::<u8>().ok();
            }
        }
    }
    None
}

// =============================================================================
// Tests — role gate (Fix 2)
// =============================================================================

#[cfg(test)]
mod role_gate_tests {
    use super::*;
    use axum::extract::{Path, State};
    use crate::hardware::{AppConfig, SimSlotSwitchRequest};
    use crate::hardware::profiles::ProfileRegistry;
    use crate::security::license::LicenseState;
    use crate::security::users::{Role, UserStore};

    async fn make_test_state() -> Arc<AppState> {
        let config = AppConfig::default();
        let users = UserStore::load("/nonexistent/users.json").await;
        let registry = ProfileRegistry::load();
        Arc::new(AppState::new(
            config,
            users,
            registry,
            "test-device-token".to_string(),
            Arc::new(crate::security::device_auth::DeviceAuth::ephemeral()),
            LicenseState::Unlicensed,
        ))
    }

    fn readonly_user() -> SessionUser {
        SessionUser { username: "viewer".to_string(), role: Role::ReadOnly }
    }

    #[tokio::test]
    async fn update_sim_slot_config_forbidden_for_readonly() {
        let state = make_test_state().await;
        let res = update_sim_slot_config(
            Path("test:mock:sim".to_string()),
            State(state),
            Extension(readonly_user()),
            Json(SimSlotConfig::default()),
        )
        .await;
        let err = res.expect_err("ReadOnly must be forbidden from SIM config write");
        assert_eq!(err.status, axum::http::StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn switch_sim_slot_forbidden_for_readonly() {
        let state = make_test_state().await;
        let res = switch_sim_slot(
            Path("test:mock:sim".to_string()),
            State(state),
            Extension(readonly_user()),
            Json(SimSlotSwitchRequest { target_slot: 2, apply_profile: false }),
        )
        .await;
        let err = res.expect_err("ReadOnly must be forbidden from SIM slot switch");
        assert_eq!(err.status, axum::http::StatusCode::FORBIDDEN);
    }
}
