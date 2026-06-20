//! User profile route handlers.
//!
//! Get and update the authenticated user's UI profile (theme, layouts, panels).
//! Any authenticated user can access their own profile.

use axum::{extract::State, Extension, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::auth_middleware::SessionUser;
use crate::api::error::{ApiError, ApiResult};
use crate::security::users::UiProfile;
use crate::state::AppState;

// === Request/Response Types ===

#[derive(Serialize)]
pub struct ProfileResponse {
    pub username: String,
    pub role: String,
    pub allowed_panels: Option<Vec<String>>,
    pub allowed_features: Option<Vec<String>>,
    pub profile: UiProfile,
}

#[derive(Deserialize)]
pub struct UpdateProfileRequest {
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub sidebar_collapsed: Option<bool>,
    #[serde(default)]
    pub layouts: Option<serde_json::Value>,
    #[serde(default)]
    pub visible_panels: Option<Vec<String>>,
    #[serde(default)]
    pub view_presets: Option<serde_json::Value>,
}

// === Handlers ===

/// GET /api/profile — get own profile.
pub async fn get_profile(
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
) -> ApiResult<Json<ProfileResponse>> {
    let profile = state
        .users
        .get_profile(&session_user.username)
        .await
        .unwrap_or_default();

    // Get allowed_panels and allowed_features for the user
    let (allowed_panels, allowed_features) = if session_user.username == "root" {
        (None, None) // Root has access to everything
    } else {
        match state.users.get_user(&session_user.username).await {
            Some(u) => (u.allowed_panels, u.allowed_features),
            None => (None, None),
        }
    };

    Ok(Json(ProfileResponse {
        username: session_user.username,
        role: session_user.role.to_string(),
        allowed_panels,
        allowed_features,
        profile,
    }))
}

/// PUT /api/profile — update own profile.
pub async fn update_profile(
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(req): Json<UpdateProfileRequest>,
) -> ApiResult<Json<ProfileResponse>> {
    // Get current profile
    let mut profile = state
        .users
        .get_profile(&session_user.username)
        .await
        .unwrap_or_default();

    // Apply updates
    if let Some(theme) = req.theme {
        profile.theme = theme;
    }
    if let Some(collapsed) = req.sidebar_collapsed {
        profile.sidebar_collapsed = collapsed;
    }
    if req.layouts.is_some() {
        profile.layouts = req.layouts;
    }
    if req.visible_panels.is_some() {
        profile.visible_panels = req.visible_panels;
    }
    if req.view_presets.is_some() {
        profile.view_presets = req.view_presets;
    }

    // Save
    state
        .users
        .update_profile(&session_user.username, profile.clone())
        .await
        .map_err(ApiError::internal)?;

    let (allowed_panels, allowed_features) = if session_user.username == "root" {
        (None, None)
    } else {
        match state.users.get_user(&session_user.username).await {
            Some(u) => (u.allowed_panels, u.allowed_features),
            None => (None, None),
        }
    };

    Ok(Json(ProfileResponse {
        username: session_user.username,
        role: session_user.role.to_string(),
        allowed_panels,
        allowed_features,
        profile,
    }))
}
