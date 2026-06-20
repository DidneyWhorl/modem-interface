//! Modem profile API route handlers.
//!
//! **Phase 1 (v1.0.0): Stubbed implementations**
//! Dynamic modem profile switching and modem management is out of scope for Phase 1.
//! These routes return minimal/stub responses. Full implementation in Phase 2.

use axum::{extract::{Path, State}, Extension, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::error::{ApiError, ApiResult};
use crate::api::auth_middleware::{require_admin, SessionUser};
use crate::hardware::DetectedModem;
use crate::state::AppState;

// ============================================================================
// Response Types
// ============================================================================

/// Summary of a modem profile for API responses.
#[derive(Debug, Clone, Serialize)]
pub struct ModemProfileSummary {
    pub profile_id: String,
    pub vendor_id: String,
    pub product_id: String,
    pub manufacturer: String,
    pub model: String,
    pub capabilities: CapabilitiesSummary,
    pub is_generic: bool,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CapabilitiesSummary {
    pub supports_5g: bool,
    pub supports_carrier_aggregation: bool,
    pub supported_technologies: Vec<String>,
    pub max_supported_bands: Vec<String>,
    pub supported_protocols: Vec<String>,
    pub has_temperature_sensor: bool,
    pub has_gps: bool,
}

/// Active modem info including profile and detection data.
#[derive(Debug, Clone, Serialize)]
pub struct ActiveModemInfo {
    pub modem_id: String,
    pub profile: ModemProfileSummary,
    pub detected: Option<DetectedModemInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DetectedModemInfo {
    pub device_path: String,
    pub description: String,
    pub protocol: String,
    pub vendor_id: Option<String>,
    pub product_id: Option<String>,
    pub bus_port: Option<String>,
}

/// Detected modem with its modem_id included.
#[derive(Debug, Clone, Serialize)]
pub struct DetectedModemWithId {
    pub modem_id: String,
    #[serde(flatten)]
    pub modem: DetectedModem,
}

#[derive(Debug, Deserialize)]
pub struct ProfileRequestData {
    pub vendor_name: String,
    pub model_name: String,
    pub usb_vendor_id: Option<String>,
    pub usb_product_id: Option<String>,
    #[allow(dead_code)]
    pub notes: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ProfileRequestResponse {
    pub success: bool,
    pub message: String,
}

// ============================================================================
// Route Handlers (Phase 1 Stubs)
// ============================================================================

/// GET /api/modem/profiles
///
/// List all available modem profiles from the registry.
pub async fn list_profiles(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<Vec<ModemProfileSummary>>> {
    let profiles = state.profile_registry.all_profiles();
    let summaries: Vec<_> = profiles.iter().map(|p| {
        ModemProfileSummary {
            profile_id: format!("{}_{}", p.identity.vendor_id, p.identity.product_id),
            vendor_id: p.identity.vendor_id.clone(),
            product_id: p.identity.product_id.clone(),
            manufacturer: p.identity.manufacturer.clone(),
            model: p.identity.model.clone(),
            capabilities: CapabilitiesSummary {
                supports_5g: p.capabilities.supports_5g,
                supports_carrier_aggregation: p.capabilities.supports_carrier_aggregation,
                supported_technologies: p.capabilities.supported_technologies.clone(),
                max_supported_bands: p.capabilities.max_supported_bands.clone(),
                supported_protocols: p.capabilities.supported_protocols.clone(),
                has_temperature_sensor: p.capabilities.has_temperature_sensor,
                has_gps: p.capabilities.has_gps,
            },
            is_generic: p.identity.vendor_id == "0000",
            notes: None,
        }
    }).collect();

    Ok(Json(summaries))
}

/// GET /api/modem/detected
///
/// List all detected modems on the system.
/// Phase 1: Returns empty list (multi-modem detection handled at startup).
pub async fn detected_modems(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<Vec<DetectedModemWithId>>> {
    // Return modems from the HashMap with their modem_id keys
    let modems = state.modems.read().await;
    let detected: Vec<DetectedModemWithId> = modems.iter()
        .map(|(id, ctx)| DetectedModemWithId {
            modem_id: id.clone(),
            modem: ctx.detected.clone(),
        })
        .collect();
    Ok(Json(detected))
}

/// POST /api/modem/rescan
///
/// Re-scan for modems and rebuild the modem list.
/// Immediately scans USB for modems, adds new ones, and removes missing ones.
pub async fn rescan_modems(
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
) -> ApiResult<Json<serde_json::Value>> {
    use tracing::{info, warn};

    // A rescan triggers a USB re-scan that rebuilds the live modem map
    // (`state.modems`) — a privileged state change. Gate it like every sibling
    // write handler (connect/reboot/band/MBN/APN) BEFORE any side effect.
    require_admin(&session_user)?;

    // Audit log the rescan action
    state
        .audit
        .log(
            crate::security::audit::AuditEventType::ConfigChanged,
            None,
            format!("Modem rescan triggered by {}", session_user.username),
        )
        .await;

    info!("Manual USB rescan requested by {}", session_user.username);

    // Step 1: USB scan (spawn_blocking to prevent blocking async runtime)
    let detected = {
        let registry = Arc::clone(&state.profile_registry);
        match tokio::task::spawn_blocking(move || crate::hardware::detect_modems(&registry, crate::hardware::DetectionVerbosity::Verbose)).await {
            Ok(modems) => modems,
            Err(e) => {
                warn!("USB scan task panicked: {}", e);
                return Err(ApiError::internal("USB scan failed"));
            }
        }
    };

    // Step 2: Generate modem IDs for all detected hardware
    let detected_with_ids: Vec<(String, crate::hardware::DetectedModem)> = detected
        .iter()
        .filter_map(|d| {
            crate::hardware::generate_modem_id(d)
                .ok()
                .map(|id| (id, d.clone()))
        })
        .collect();

    // Step 3: Identify changes
    let current_ids: std::collections::HashSet<String> = {
        let modems = state.modems.read().await;
        modems.keys().cloned().collect()
    };

    let detected_ids: std::collections::HashSet<String> = detected_with_ids
        .iter()
        .map(|(id, _)| id.clone())
        .collect();

    let to_add: Vec<_> = detected_with_ids
        .iter()
        .filter(|(id, _)| !current_ids.contains(id))
        .collect();

    let to_remove: Vec<String> = current_ids
        .iter()
        .filter(|id| !detected_ids.contains(*id))
        .cloned()
        .collect();

    let add_count = to_add.len();
    let remove_count = to_remove.len();

    info!(
        "Rescan: {} to add, {} to remove",
        add_count,
        remove_count
    );

    // Step 4: Add new modems (same logic as hot-plug watcher)
    for (modem_id, detected) in to_add {
        info!("[{}] Adding modem from rescan", modem_id);

        // Match profile
        let profile = match (&detected.vendor_id, &detected.product_id) {
            (Some(vid), Some(pid)) => state.profile_registry.match_profile(vid, pid).clone(),
            _ => state.profile_registry.generic().clone(),
        };

        // Create handler (30s timeout)
        let detected_clone = detected.clone();
        let profile_clone = profile.clone();
        let handler_task = tokio::task::spawn_blocking(move || {
            crate::hardware::create_modem_handler(&detected_clone, profile_clone)
        });

        let handler = match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            handler_task,
        )
        .await
        {
            Ok(Ok(Ok(h))) => h,
            Ok(Ok(Err(e))) => {
                warn!("[{}] Handler creation failed: {}, skipping", modem_id, e);
                continue;
            }
            Ok(Err(e)) => {
                warn!("[{}] Handler task panicked: {}, skipping", modem_id, e);
                continue;
            }
            Err(_) => {
                warn!("[{}] Handler creation timed out, skipping", modem_id);
                continue;
            }
        };

        // Run discovery (15s timeout with fallback)
        let handler_arc = Arc::new(tokio::sync::Mutex::new(handler));
        let handler_clone = Arc::clone(&handler_arc);

        let discovery = match tokio::time::timeout(
            std::time::Duration::from_secs(15),
            async move {
                let modem = handler_clone.lock().await;
                modem.get_discovery_info().await
            },
        )
        .await
        {
            Ok(Ok(info)) => info,
            Ok(Err(e)) => {
                warn!("[{}] Discovery failed: {}, using defaults", modem_id, e);
                crate::hardware::DiscoveryInfo::default()
            }
            Err(_) => {
                warn!("[{}] Discovery timed out, using defaults", modem_id);
                crate::hardware::DiscoveryInfo::default()
            }
        };

        // Extract handler from Arc
        let handler = match Arc::try_unwrap(handler_arc) {
            Ok(mutex) => mutex.into_inner(),
            Err(_) => {
                warn!("[{}] Failed to unwrap handler Arc, skipping", modem_id);
                continue;
            }
        };

        // Add to state
        let config = {
            let config = state.config.read().await;
            config.connection.clone()
        };

        state
            .add_modem(
                modem_id.clone(),
                handler,
                profile,
                detected.clone(),
                config,
                discovery,
            )
            .await;

        // USB-net mode detection (diagnostic only; never blocks bring-up).
        // Per spec §3.10 detect_usbnet_mode never returns Err; failure cached as Unknown.
        state.detect_and_cache_usbnet_mode(modem_id).await;

        // Broadcast event
        state.broadcast_modem_event(
            modem_id,
            crate::hardware::ModemEvent::ModemHealth(crate::hardware::ModemHealth {
                available: true,
                state: crate::hardware::ModemHealthState::Ok,
                message: Some("Added via rescan".to_string()),
            }),
        );
    }

    // Step 5: Remove missing modems (immediate, no 5min grace like watcher)
    for modem_id in to_remove {
        info!("[{}] Removing modem from rescan", modem_id);

        state.remove_modem(&modem_id).await;

        state.broadcast_modem_event(
            &modem_id,
            crate::hardware::ModemEvent::ModemHealth(crate::hardware::ModemHealth {
                available: false,
                state: crate::hardware::ModemHealthState::Error,
                message: Some("Removed via rescan".to_string()),
            }),
        );
    }

    // Step 6: Update detected_modems list
    {
        let mut detected_modems = state.detected_modems.write().await;
        *detected_modems = detected;
    }

    // Step 7: Return summary
    let final_count = state.modems.read().await.len();

    info!(
        "Rescan complete: {} total modems ({} added, {} removed)",
        final_count,
        add_count,
        remove_count
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "modem_count": final_count,
        "added": add_count,
        "removed": remove_count,
        "message": format!("{} total ({} added, {} removed)",
            final_count, add_count, remove_count)
    })))
}

/// GET /api/modem/:modem_id/profile
///
/// Get the active profile for the specified modem.
pub async fn active_profile(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<ActiveModemInfo>> {
    let modems = state.modems.read().await;
    let context = modems.get(&modem_id).ok_or_else(|| {
        ApiError::not_found(format!("Modem not found: {modem_id}"))
    })?;

    let profile = &context.profile;
    let detected = &context.detected;

    Ok(Json(ActiveModemInfo {
        modem_id: modem_id.clone(),
        profile: ModemProfileSummary {
            profile_id: format!("{}_{}", profile.identity.vendor_id, profile.identity.product_id),
            vendor_id: profile.identity.vendor_id.clone(),
            product_id: profile.identity.product_id.clone(),
            manufacturer: profile.identity.manufacturer.clone(),
            model: profile.identity.model.clone(),
            capabilities: CapabilitiesSummary {
                supports_5g: profile.capabilities.supports_5g,
                supports_carrier_aggregation: profile.capabilities.supports_carrier_aggregation,
                supported_technologies: profile.capabilities.supported_technologies.clone(),
                max_supported_bands: profile.capabilities.max_supported_bands.clone(),
                supported_protocols: profile.capabilities.supported_protocols.clone(),
                has_temperature_sensor: profile.capabilities.has_temperature_sensor,
                has_gps: profile.capabilities.has_gps,
            },
            is_generic: profile.identity.vendor_id == "0000",
            notes: None,
        },
        detected: Some(DetectedModemInfo {
            device_path: detected.device_path.clone(),
            description: detected.description.clone(),
            protocol: format!("{:?}", detected.protocol),
            vendor_id: detected.vendor_id.clone(),
            product_id: detected.product_id.clone(),
            bus_port: detected.bus_port.clone(),
        }),
    }))
}

/// POST /api/modem/:modem_id/profile/override
///
/// Override the active profile for dynamic testing.
/// Phase 1: Not implemented (requires handler recreation).
pub async fn override_profile(
    Path(_modem_id): Path<String>,
    State(_state): State<Arc<AppState>>,
    Extension(_session_user): Extension<SessionUser>,
    Json(_req): Json<serde_json::Value>,
) -> ApiResult<Json<serde_json::Value>> {
    Err(ApiError::bad_request(
        "Profile override not implemented in Phase 1. Modem profiles are static."
    ))
}

/// POST /api/modem/:modem_id/profile/request
///
/// Submit a request for a new modem profile to be added.
pub async fn request_profile(
    Path(_modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(req): Json<ProfileRequestData>,
) -> ApiResult<Json<ProfileRequestResponse>> {
    state
        .audit
        .log(
            crate::security::audit::AuditEventType::ConfigChanged,
            None,
            format!(
                "{} requested profile: {} {} (VID:{:?}, PID:{:?})",
                session_user.username, req.vendor_name, req.model_name, req.usb_vendor_id, req.usb_product_id
            ),
        )
        .await;

    Ok(Json(ProfileRequestResponse {
        success: true,
        message: format!(
            "Profile request for {} {} submitted. Submit modem details to the project repository.",
            req.vendor_name, req.model_name
        ),
    }))
}

/// POST /api/modem/:modem_id/discover
///
/// Run modem discovery and save results to /tmp/ for profile creation.
/// Phase 1: Stub (discovery logic exists but needs integration).
pub async fn discover_modem(
    Path(_modem_id): Path<String>,
    State(_state): State<Arc<AppState>>,
    Extension(_session_user): Extension<SessionUser>,
) -> ApiResult<Json<serde_json::Value>> {
    Err(ApiError::bad_request(
        "Modem discovery not implemented in Phase 1. Discovery runs automatically at startup for generic modems."
    ))
}

// ============================================================================
// Backward-Compat Route Handlers (Old Single-Modem API)
// ============================================================================

/// GET /api/modem/profile/active (backward-compat)
///
/// Get the active profile for the currently selected modem.
/// Uses selected_modem_id from state, defaults to first modem if none selected.
pub async fn active_profile_compat(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<ActiveModemInfo>> {
    let modem_id = state.get_selected_or_first_modem().await
        .ok_or_else(|| ApiError::not_found("No modems available"))?;
    active_profile(Path(modem_id), State(state)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::AppConfig;
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

    fn admin_user() -> SessionUser {
        SessionUser {
            username: "admin".to_string(),
            role: Role::Admin,
        }
    }

    fn readonly_user() -> SessionUser {
        SessionUser {
            username: "viewer".to_string(),
            role: Role::ReadOnly,
        }
    }

    // FIX 2: rescan_modems rebuilds the live modem map and must be Admin-gated.

    #[tokio::test]
    async fn rescan_forbidden_for_readonly() {
        let state = make_test_state().await;
        let res = rescan_modems(State(state), Extension(readonly_user())).await;
        let err = res.expect_err("ReadOnly must be forbidden from rescan");
        assert_eq!(err.status, axum::http::StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn rescan_allows_admin() {
        // Admin passes the gate; the USB scan finds no hardware on the test host
        // and the handler returns a successful (empty) summary.
        let state = make_test_state().await;
        let res = rescan_modems(State(state), Extension(admin_user())).await;
        let Json(body) = res.expect("Admin rescan must return Ok");
        assert_eq!(body["success"], serde_json::json!(true));
    }
}
