//! Configuration API route handlers.
//!
//! Handlers for /api/config endpoints for persistent settings.

use axum::{
    extract::{Path, State},
    Extension, Json,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

use crate::api::auth_middleware::{require_admin, SessionUser};
use crate::api::error::{ApiError, ApiResult};
use crate::hardware::{ConnectionConfig, IpType};
use crate::state::AppState;

/// GET /api/modem/:modem_id/config
///
/// Get per-modem connection configuration.
/// When the connection APN is empty, queries the modem's PDP context
/// (AT+CGDCONT?) to populate it from the active connection settings.
pub async fn get_config(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<ConnectionConfig>> {
    let modems = state.modems.read().await;
    let context = modems.get(&modem_id).ok_or_else(|| {
        ApiError::not_found(format!("Modem not found: {modem_id}"))
    })?;

    let mut config = context.config.read().await.clone();
    let handler_arc = Arc::clone(&context.handler);
    drop(modems);

    // If APN is empty, try to read it from the modem's active PDP context
    if config.apn.is_empty() {
        if let Ok(modem) = timeout(Duration::from_secs(2), handler_arc.lock()).await {
            if let Ok(Ok(response)) =
                timeout(Duration::from_secs(3), modem.execute_at("AT+CGDCONT?")).await
            {
                // Prefer the user's configured CID, fall back to first non-empty
                let target_cid = config.cid;
                if let Some((apn, ip_type)) = parse_cgdcont(&response, target_cid) {
                    config.apn = apn.clone();
                    config.ip_type = ip_type;
                    // Cache in modem context so we don't re-query every time
                    let modems = state.modems.read().await;
                    if let Some(context) = modems.get(&modem_id) {
                        let mut cfg = context.config.write().await;
                        cfg.apn = apn;
                        cfg.ip_type = ip_type;
                    }
                }
            }
        }
    }

    Ok(Json(config))
}

/// Parse AT+CGDCONT? response to extract APN and IP type.
/// Format: +CGDCONT: <cid>,"<pdp_type>","<apn>",...
/// Prefers the context matching `target_cid`, falls back to the first
/// non-empty APN if the target CID has no APN set.
fn parse_cgdcont(response: &str, target_cid: u8) -> Option<(String, IpType)> {
    let mut fallback: Option<(String, IpType)> = None;

    for line in response.lines() {
        let line = line.trim();
        if !line.starts_with("+CGDCONT:") {
            continue;
        }
        // Strip prefix to get: <cid>,"<pdp_type>","<apn>",...
        let after_prefix = line.strip_prefix("+CGDCONT:")?.trim();
        let parts: Vec<&str> = after_prefix.split(',').collect();
        if parts.len() >= 3 {
            let cid: u8 = parts[0].trim().parse().ok()?;
            let pdp_type = parts[1].trim().trim_matches('"');
            let apn = parts[2].trim().trim_matches('"');
            if apn.is_empty() {
                continue;
            }
            let ip_type = match pdp_type.to_uppercase().as_str() {
                "IPV6" => IpType::Ipv6,
                "IPV4V6" => IpType::Ipv4v6,
                _ => IpType::Ipv4,
            };
            if cid == target_cid {
                return Some((apn.to_string(), ip_type));
            }
            if fallback.is_none() {
                fallback = Some((apn.to_string(), ip_type));
            }
        }
    }
    fallback
}

/// PUT /api/modem/:modem_id/config
///
/// Update per-modem connection configuration.
pub async fn update_config(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(new_config): Json<ConnectionConfig>,
) -> ApiResult<Json<ConnectionConfig>> {
    require_admin(&session_user)?;

    // Validate configuration
    if new_config.apn.len() > 100 {
        return Err(ApiError::bad_request("APN too long"));
    }
    if new_config.cid == 0 || new_config.cid > 8 {
        return Err(ApiError::bad_request("CID must be 1-8"));
    }

    let modems = state.modems.read().await;
    let context = modems.get(&modem_id).ok_or_else(|| {
        ApiError::not_found(format!("Modem not found: {modem_id}"))
    })?;

    let mut config = context.config.write().await;
    *config = new_config.clone();
    drop(config);
    drop(modems);

    // TODO: Per-modem config persistence
    // Will be implemented to save to /etc/modem-interface/modems/{modem_id}.toml

    Ok(Json(new_config))
}
