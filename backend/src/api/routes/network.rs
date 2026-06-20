//! Network API route handlers.
//!
//! Handlers for /api/network/* endpoints including scan, selection, and registration.

use axum::{
    extract::{Path, State},
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

use crate::api::auth_middleware::{require_admin, SessionUser};
use crate::api::error::{ApiError, ApiResult};
use crate::hardware::{AvailableNetwork, RegistrationState};
use crate::state::AppState;

const STATE_CHANGE_TIMEOUT: Duration = Duration::from_secs(15);
const LONG_TIMEOUT: Duration = Duration::from_secs(60);

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

/// GET /api/modem/:modem_id/network/scan
///
/// Scan for available networks. This is a slow operation (30-60+ seconds)
/// and will temporarily disconnect from the current network.
pub async fn scan(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<Vec<AvailableNetwork>>> {
    let handler_arc = get_modem_handler(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    let networks = timeout(LONG_TIMEOUT, modem.scan_networks())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Network scan timed out", 30))?
        .map_err(ApiError::from)?;

    Ok(Json(networks))
}

/// Network selection request body.
#[derive(Debug, Deserialize)]
pub struct SelectRequest {
    /// Operator code (MCC+MNC). Omit or set to null for automatic selection.
    pub operator_code: Option<String>,
}

/// POST /api/modem/:modem_id/network/select
///
/// Manually select a network. Omit operator_code for automatic selection.
pub async fn select(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(req): Json<SelectRequest>,
) -> ApiResult<Json<SelectionResponse>> {
    require_admin(&session_user)?;
    let handler_arc = get_modem_handler(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    timeout(
        STATE_CHANGE_TIMEOUT,
        modem.select_network(req.operator_code.as_deref()),
    )
    .await
    .map_err(|_| ApiError::service_unavailable_with_retry("Network selection timed out", 5))?
    .map_err(ApiError::from)?;

    let message = if req.operator_code.is_some() {
        "Network selection initiated".to_string()
    } else {
        "Automatic network selection enabled".to_string()
    };

    Ok(Json(SelectionResponse {
        success: true,
        message,
    }))
}

#[derive(Debug, Serialize)]
pub struct SelectionResponse {
    pub success: bool,
    pub message: String,
}

/// GET /api/modem/:modem_id/network/registration
///
/// Get current network registration state.
pub async fn registration(
    Path(modem_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<RegistrationState>> {
    let handler_arc = get_modem_handler(&state, &modem_id).await?;
    let modem = timeout(Duration::from_secs(2), handler_arc.lock())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Modem busy", 1))?;

    let reg = timeout(Duration::from_secs(5), modem.get_registration())
        .await
        .map_err(|_| ApiError::service_unavailable_with_retry("Request timed out", 1))?
        .map_err(ApiError::from)?;

    Ok(Json(reg))
}
