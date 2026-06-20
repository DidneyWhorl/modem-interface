//! User management route handlers.
//!
//! CRUD operations for user accounts. Requires Admin or SuperAdmin role.
//! Role enforcement: Admins can only manage ReadOnly users.
//! SuperAdmins can manage all users except cannot delete root.

use axum::{
    extract::{Path, State},
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::info;

use crate::api::auth_middleware::SessionUser;
use crate::api::error::{ApiError, ApiResult};
use crate::security::audit::AuditEventType;
use crate::security::users::{password_meets_min_len, Role, UiProfile, User, UserInfo};
use crate::state::AppState;

// === Request/Response Types ===

#[derive(Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    #[serde(default = "default_role")]
    pub role: Role,
    #[serde(default)]
    pub allowed_panels: Option<Vec<String>>,
    #[serde(default)]
    pub allowed_features: Option<Vec<String>>,
}

fn default_role() -> Role {
    Role::ReadOnly
}

#[derive(Deserialize)]
pub struct UpdateUserRequest {
    #[serde(default)]
    pub role: Option<Role>,
    #[serde(default)]
    pub allowed_panels: Option<Option<Vec<String>>>,
    #[serde(default)]
    pub allowed_features: Option<Option<Vec<String>>>,
    #[serde(default)]
    pub disabled: Option<bool>,
}

#[derive(Deserialize)]
pub struct ResetPasswordRequest {
    pub new_password: String,
}

#[derive(Serialize)]
pub struct UserListResponse {
    pub users: Vec<UserInfo>,
}

#[derive(Serialize)]
pub struct SuccessResponse {
    pub success: bool,
}

// === Helpers ===

/// Check if the caller has at least Admin role.
fn require_admin(session_user: &SessionUser) -> Result<(), ApiError> {
    if session_user.role < Role::Admin {
        return Err(ApiError::forbidden("Admin access required"));
    }
    Ok(())
}

/// Check if the caller can manage the target user's role.
fn can_manage_role(caller: &SessionUser, target_role: Role) -> bool {
    match caller.role {
        Role::SuperAdmin => true,
        Role::Admin => target_role < Role::Admin, // Admins can only manage ReadOnly
        Role::ReadOnly => false,
    }
}

/// Hash a password with argon2id.
fn hash_password(password: &str) -> Result<String, ApiError> {
    use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
    use rand::rngs::OsRng;

    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| ApiError::internal(format!("Failed to hash password: {e}")))
}

// === Handlers ===

/// GET /api/users — list all users.
pub async fn list_users(
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
) -> ApiResult<Json<UserListResponse>> {
    require_admin(&session_user)?;

    let users = state.users.list_users().await;
    Ok(Json(UserListResponse { users }))
}

/// POST /api/users — create a new user.
pub async fn create_user(
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Json(req): Json<CreateUserRequest>,
) -> ApiResult<Json<SuccessResponse>> {
    require_admin(&session_user)?;

    // Admins can only create ReadOnly users
    if !can_manage_role(&session_user, req.role) {
        return Err(ApiError::forbidden(
            "You cannot create users with that role",
        ));
    }

    if !password_meets_min_len(&req.password) {
        return Err(ApiError::bad_request(
            "Password must be at least 12 characters",
        ));
    }

    let hash = hash_password(&req.password)?;

    // Default read-only users to connection-status + signal if no panels specified
    let allowed_panels = if req.role == Role::ReadOnly && req.allowed_panels.is_none() {
        Some(vec![
            "connection-status".to_string(),
            "signal".to_string(),
        ])
    } else {
        req.allowed_panels
    };

    let user = User {
        username: req.username.clone(),
        role: req.role,
        password_hash: Some(hash),
        allowed_panels,
        allowed_features: req.allowed_features,
        ui_profile: UiProfile::default(),
        disabled: false,
    };

    state
        .users
        .create_user(user)
        .await
        .map_err(ApiError::bad_request)?;

    info!(
        "{} created user '{}' with role {:?}",
        session_user.username, req.username, req.role
    );
    state
        .audit
        .log(
            AuditEventType::UserCreated,
            None,
            format!(
                "{} created user '{}' ({:?})",
                session_user.username, req.username, req.role
            ),
        )
        .await;

    Ok(Json(SuccessResponse { success: true }))
}

/// GET /api/users/:username — get user details.
pub async fn get_user(
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Path(username): Path<String>,
) -> ApiResult<Json<UserInfo>> {
    require_admin(&session_user)?;

    let user = state
        .users
        .get_user(&username)
        .await
        .ok_or_else(|| ApiError::not_found(format!("User '{username}' not found")))?;

    Ok(Json(UserInfo::from(&user)))
}

/// PUT /api/users/:username — update user.
pub async fn update_user(
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Path(username): Path<String>,
    Json(req): Json<UpdateUserRequest>,
) -> ApiResult<Json<SuccessResponse>> {
    require_admin(&session_user)?;

    if username == "root" {
        return Err(ApiError::forbidden("Cannot modify root user"));
    }

    // Prevent users from disabling their own account
    if username == session_user.username {
        if let Some(true) = req.disabled {
            return Err(ApiError::bad_request("Cannot disable your own account"));
        }
    }

    // Check the target user exists and get their current role
    let target = state
        .users
        .get_user(&username)
        .await
        .ok_or_else(|| ApiError::not_found(format!("User '{username}' not found")))?;

    if !can_manage_role(&session_user, target.role) {
        return Err(ApiError::forbidden(
            "You cannot modify users with that role",
        ));
    }

    // If changing role, validate the new role too
    if let Some(new_role) = req.role {
        if !can_manage_role(&session_user, new_role) {
            return Err(ApiError::forbidden(
                "You cannot assign that role",
            ));
        }
    }

    state
        .users
        .update_user(&username, req.role, req.allowed_panels, req.allowed_features, req.disabled)
        .await
        .map_err(ApiError::bad_request)?;

    info!("{} updated user '{username}'", session_user.username);
    state
        .audit
        .log(
            AuditEventType::UserUpdated,
            None,
            format!("{} updated user '{username}'", session_user.username),
        )
        .await;

    Ok(Json(SuccessResponse { success: true }))
}

/// DELETE /api/users/:username — delete user.
pub async fn delete_user(
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Path(username): Path<String>,
) -> ApiResult<Json<SuccessResponse>> {
    require_admin(&session_user)?;

    if username == "root" {
        return Err(ApiError::forbidden("Cannot delete root user"));
    }

    if username == session_user.username {
        return Err(ApiError::bad_request("Cannot delete your own account"));
    }

    // Check the target user's role
    let target = state
        .users
        .get_user(&username)
        .await
        .ok_or_else(|| ApiError::not_found(format!("User '{username}' not found")))?;

    if !can_manage_role(&session_user, target.role) {
        return Err(ApiError::forbidden(
            "You cannot delete users with that role",
        ));
    }

    state
        .users
        .delete_user(&username)
        .await
        .map_err(ApiError::bad_request)?;

    info!("{} deleted user '{username}'", session_user.username);
    state
        .audit
        .log(
            AuditEventType::UserDeleted,
            None,
            format!("{} deleted user '{username}'", session_user.username),
        )
        .await;

    Ok(Json(SuccessResponse { success: true }))
}

/// POST /api/users/:username/reset-password — reset a user's password.
pub async fn reset_password(
    State(state): State<Arc<AppState>>,
    Extension(session_user): Extension<SessionUser>,
    Path(username): Path<String>,
    Json(req): Json<ResetPasswordRequest>,
) -> ApiResult<Json<SuccessResponse>> {
    require_admin(&session_user)?;

    if username == "root" {
        return Err(ApiError::forbidden(
            "Root password must be changed via SSH using 'passwd'",
        ));
    }

    let target = state
        .users
        .get_user(&username)
        .await
        .ok_or_else(|| ApiError::not_found(format!("User '{username}' not found")))?;

    if !can_manage_role(&session_user, target.role) {
        return Err(ApiError::forbidden(
            "You cannot reset passwords for users with that role",
        ));
    }

    if !password_meets_min_len(&req.new_password) {
        return Err(ApiError::bad_request(
            "Password must be at least 12 characters",
        ));
    }

    let hash = hash_password(&req.new_password)?;
    state
        .users
        .set_password_hash(&username, hash)
        .await
        .map_err(ApiError::internal)?;

    info!(
        "{} reset password for '{username}'",
        session_user.username
    );
    state
        .audit
        .log(
            AuditEventType::PasswordReset,
            None,
            format!(
                "{} reset password for '{username}'",
                session_user.username
            ),
        )
        .await;

    Ok(Json(SuccessResponse { success: true }))
}
