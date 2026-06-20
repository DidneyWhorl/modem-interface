//! Authentication route handlers.
//!
//! Provides multi-user login, logout, status check, first-run setup,
//! and password change. Login and setup endpoints are rate-limited per IP.

use axum::extract::connect_info::ConnectInfo;
use axum::{extract::State, Extension, Json};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::{info, warn};

use crate::api::auth_middleware::SessionUser;
use crate::api::error::{ApiError, ApiResult};
use crate::security::audit::AuditEventType;
use crate::security::rate_limit::RateCategory;
use crate::security::users::{password_meets_min_len, Role, UiProfile, User};
use crate::state::AppState;

/// Cookie name for session token.
pub const SESSION_COOKIE: &str = "modem_session";

// === Request/Response Types ===

#[derive(Deserialize)]
pub struct LoginRequest {
    #[serde(default = "default_username")]
    pub username: String,
    pub password: String,
}

fn default_username() -> String {
    "root".to_string()
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

#[derive(Serialize)]
pub struct AuthStatusResponse {
    pub authenticated: bool,
    pub auth_required: bool,
    pub setup_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

#[derive(Deserialize)]
pub struct SetupRequest {
    #[serde(default = "default_setup_username")]
    pub username: String,
    pub password: String,
}

fn default_setup_username() -> String {
    "admin".to_string()
}

#[derive(Serialize)]
pub struct SetupResponse {
    pub success: bool,
}

#[derive(Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Serialize)]
pub struct ChangePasswordResponse {
    pub success: bool,
}

// === Helpers ===

/// Extract client IP from ConnectInfo, if available.
fn client_ip(info: &Option<ConnectInfo<SocketAddr>>) -> Option<std::net::IpAddr> {
    info.as_ref().map(|ci| ci.0.ip())
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

/// Verify a password against an argon2id hash.
fn verify_argon2(password: &str, hash: &str) -> bool {
    use argon2::{Argon2, PasswordHash, PasswordVerifier};
    match PasswordHash::new(hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

// === Handlers ===

/// POST /api/auth/login — authenticate with username + password.
pub async fn login(
    State(state): State<Arc<AppState>>,
    info: Option<ConnectInfo<SocketAddr>>,
    jar: CookieJar,
    Json(req): Json<LoginRequest>,
) -> ApiResult<(CookieJar, Json<LoginResponse>)> {
    let ip = client_ip(&info);

    // Rate limit check
    if let Some(ip_addr) = ip {
        if let Err(retry_after) = state.rate_limiter.check(ip_addr, RateCategory::Login).await {
            state
                .audit
                .log(
                    AuditEventType::RateLimited,
                    Some(ip_addr),
                    "Login rate limit exceeded",
                )
                .await;
            return Err(ApiError::rate_limited(
                "Too many login attempts. Try again later.",
                retry_after,
            ));
        }
    }

    // Per-account lockout check (complements the per-IP limiter above). Keyed on
    // the submitted username so an IP-rotating attacker can't get unlimited
    // guesses against a single account (notably `root`). Temporary/self-clearing
    // — never a permanent lock, so `root` always recovers. If currently locked,
    // reject WITHOUT attempting the password check.
    if let Some(remaining) = state.login_lockout.check_locked(&req.username).await {
        state
            .audit
            .log(
                AuditEventType::RateLimited,
                ip,
                format!("Account temporarily locked: {}", req.username),
            )
            .await;
        return Err(ApiError::rate_limited(
            "Account temporarily locked due to repeated failures. Try again later.",
            remaining.as_secs().max(1),
        ));
    }

    // Authenticate based on username
    let (username, role) = if req.username == "root" {
        // Root authenticates via /etc/shadow
        match crate::security::shadow::verify_shadow_password("root", &req.password) {
            Ok(true) => ("root".to_string(), Role::SuperAdmin),
            Ok(false) => {
                warn!("Failed login attempt for root");
                state.login_lockout.record_failure(&req.username).await;
                state
                    .audit
                    .log(
                        AuditEventType::LoginFailure,
                        ip,
                        "Invalid password for root",
                    )
                    .await;
                return Err(ApiError::unauthorized("Invalid credentials"));
            }
            Err(e) => {
                warn!("Shadow auth unavailable: {e}");
                // Shadow not available — root login not possible
                state
                    .audit
                    .log(
                        AuditEventType::LoginFailure,
                        ip,
                        format!("Shadow auth failed: {e}"),
                    )
                    .await;
                return Err(ApiError::unauthorized("Invalid credentials"));
            }
        }
    } else {
        // Regular user authenticates via UserStore
        let user = state
            .users
            .get_user(&req.username)
            .await
            .ok_or_else(|| ApiError::unauthorized("Invalid credentials"))?;

        if user.disabled {
            state
                .audit
                .log(
                    AuditEventType::LoginFailure,
                    ip,
                    format!("Disabled account: {}", req.username),
                )
                .await;
            return Err(ApiError::unauthorized(
                "Your account has been deactivated. Please contact your administrator.",
            ));
        }

        let hash = user
            .password_hash
            .as_ref()
            .ok_or_else(|| ApiError::unauthorized("Invalid credentials"))?;

        if !verify_argon2(&req.password, hash) {
            warn!("Failed login attempt for {}", req.username);
            state.login_lockout.record_failure(&req.username).await;
            state
                .audit
                .log(
                    AuditEventType::LoginFailure,
                    ip,
                    format!("Invalid password for {}", req.username),
                )
                .await;
            return Err(ApiError::unauthorized("Invalid credentials"));
        }

        (user.username, user.role)
    };

    // Successful credential check — clear any accumulated lockout counter for
    // this account (keyed on the submitted username, matching the failure keys).
    state.login_lockout.record_success(&req.username).await;

    // Check if TLS is actually serving (not just configured)
    let tls_active = state.tls_active.load(std::sync::atomic::Ordering::Relaxed);

    // Create session with user identity
    let token = state.sessions.create(username.clone(), role).await;
    info!("Login successful for {username}");
    state
        .audit
        .log(
            AuditEventType::LoginSuccess,
            ip,
            format!("Session created for {username}"),
        )
        .await;

    let cookie = Cookie::build((SESSION_COOKIE, token))
        .path("/ctrl-modem")
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(tls_active)
        .build();

    Ok((
        jar.add(cookie),
        Json(LoginResponse {
            success: true,
            username: Some(username),
            role: Some(role.to_string()),
        }),
    ))
}

/// POST /api/auth/logout — clear session.
pub async fn logout(
    State(state): State<Arc<AppState>>,
    info: Option<ConnectInfo<SocketAddr>>,
    jar: CookieJar,
) -> (CookieJar, Json<LoginResponse>) {
    let ip = client_ip(&info);

    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        state.sessions.remove(cookie.value()).await;
    }

    state
        .audit
        .log(AuditEventType::Logout, ip, "Session ended")
        .await;

    let removal = Cookie::build((SESSION_COOKIE, ""))
        .path("/ctrl-modem")
        .http_only(true)
        .removal()
        .build();

    (
        jar.remove(removal),
        Json(LoginResponse {
            success: true,
            username: None,
            role: None,
        }),
    )
}

/// GET /api/auth/status — check current auth state.
pub async fn status(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> Json<AuthStatusResponse> {
    let config = state.config.read().await;
    let auth_required = config.auth.enabled;

    // Setup required if no users exist (and no legacy password_hash)
    let has_users = state.users.has_users().await || config.auth.password_hash.is_some();
    let setup_required = auth_required && !has_users;

    // Check session
    let (authenticated, username, role) = if !auth_required {
        (true, None, None)
    } else if setup_required {
        (false, None, None)
    } else if let Some(cookie) = jar.get(SESSION_COOKIE) {
        match state.sessions.validate(cookie.value()).await {
            Some(info) => (true, Some(info.username), Some(info.role.to_string())),
            None => (false, None, None),
        }
    } else {
        (false, None, None)
    };

    Json(AuthStatusResponse {
        authenticated,
        auth_required,
        setup_required,
        username,
        role,
    })
}

/// POST /api/auth/setup — first-run account creation.
/// Only works when no users are configured yet.
pub async fn setup(
    State(state): State<Arc<AppState>>,
    info: Option<ConnectInfo<SocketAddr>>,
    Json(req): Json<SetupRequest>,
) -> ApiResult<Json<SetupResponse>> {
    let ip = client_ip(&info);

    // Rate limit check
    if let Some(ip_addr) = ip {
        if let Err(retry_after) = state.rate_limiter.check(ip_addr, RateCategory::Setup).await {
            state
                .audit
                .log(
                    AuditEventType::RateLimited,
                    Some(ip_addr),
                    "Setup rate limit exceeded",
                )
                .await;
            return Err(ApiError::rate_limited(
                "Too many setup attempts. Try again later.",
                retry_after,
            ));
        }
    }

    // Only allow if no users exist yet
    if state.users.has_users().await {
        return Err(ApiError::forbidden("Users already configured"));
    }
    {
        let config = state.config.read().await;
        if config.auth.password_hash.is_some() {
            return Err(ApiError::forbidden("Password already configured"));
        }
    }

    if !password_meets_min_len(&req.password) {
        return Err(ApiError::bad_request(
            "Password must be at least 12 characters",
        ));
    }

    let username = if req.username.is_empty() {
        "admin".to_string()
    } else {
        req.username
    };

    let hash = hash_password(&req.password)?;

    let user = User {
        username: username.clone(),
        role: Role::Admin,
        password_hash: Some(hash),
        allowed_panels: None,
        allowed_features: None,
        ui_profile: UiProfile::default(),
        disabled: false,
    };

    state
        .users
        .create_user(user)
        .await
        .map_err(ApiError::bad_request)?;

    info!("Initial setup complete: created user '{username}'");
    state
        .audit
        .log(
            AuditEventType::SetupComplete,
            ip,
            format!("User '{username}' created during setup"),
        )
        .await;
    Ok(Json(SetupResponse { success: true }))
}

/// POST /api/auth/change-password — change own password (requires auth).
pub async fn change_password(
    State(state): State<Arc<AppState>>,
    info: Option<ConnectInfo<SocketAddr>>,
    Extension(session_user): Extension<SessionUser>,
    Json(req): Json<ChangePasswordRequest>,
) -> ApiResult<Json<ChangePasswordResponse>> {
    let ip = client_ip(&info);

    // Root cannot change password here
    if session_user.username == "root" {
        return Err(ApiError::bad_request(
            "Root password must be changed via SSH using 'passwd'",
        ));
    }

    if !password_meets_min_len(&req.new_password) {
        return Err(ApiError::bad_request(
            "Password must be at least 12 characters",
        ));
    }

    // Verify current password
    let user = state
        .users
        .get_user(&session_user.username)
        .await
        .ok_or_else(|| ApiError::internal("Session user not found"))?;

    let current_hash = user
        .password_hash
        .as_ref()
        .ok_or_else(|| ApiError::internal("User has no password"))?;

    if !verify_argon2(&req.current_password, current_hash) {
        state
            .audit
            .log(
                AuditEventType::LoginFailure,
                ip,
                format!(
                    "Wrong current password during change for {}",
                    session_user.username
                ),
            )
            .await;
        return Err(ApiError::unauthorized("Current password is incorrect"));
    }

    // Hash and save new password
    let new_hash = hash_password(&req.new_password)?;
    state
        .users
        .set_password_hash(&session_user.username, new_hash)
        .await
        .map_err(ApiError::internal)?;

    info!("Password changed for {}", session_user.username);
    state
        .audit
        .log(
            AuditEventType::PasswordChanged,
            ip,
            format!("Password changed for {}", session_user.username),
        )
        .await;

    Ok(Json(ChangePasswordResponse { success: true }))
}

// === WebSocket Token ===

#[derive(Serialize)]
pub struct WsTokenResponse {
    pub token: String,
}

/// POST /api/auth/ws-token — issue a short-lived, single-use WebSocket auth token.
///
/// Requires a valid session (auth middleware injects SessionUser).
/// The returned token is valid for 30 seconds and can only be used once.
/// Rate limited to 10 tokens per minute per session (enforced by WsTokenStore).
pub async fn ws_token(
    State(state): State<Arc<AppState>>,
    info: Option<ConnectInfo<SocketAddr>>,
    Extension(session_user): Extension<SessionUser>,
) -> ApiResult<Json<WsTokenResponse>> {
    let ip = client_ip(&info);

    match state
        .ws_tokens
        .create(session_user.username.clone(), session_user.role)
        .await
    {
        Ok(token) => {
            tracing::debug!("WS token issued for {}", session_user.username);
            Ok(Json(WsTokenResponse { token }))
        }
        Err(retry_after) => {
            state
                .audit
                .log(
                    AuditEventType::RateLimited,
                    ip,
                    format!(
                        "WS token rate limit exceeded for {}",
                        session_user.username
                    ),
                )
                .await;
            Err(ApiError::rate_limited(
                "Too many WebSocket token requests",
                retry_after,
            ))
        }
    }
}

// =============================================================================
// Login lockout tests (FIX 1 — per-account temporary lockout wiring)
// =============================================================================
//
// These exercise the lockout wired into `login`. They drive the deterministic
// regular-user (UserStore) path — no /etc/shadow dependency — and pass
// `info: None` so the per-IP rate limiter is skipped (it only engages when a
// client IP is present), isolating the account-keyed lockout under test.

#[cfg(test)]
mod lockout_tests {
    use super::*;
    use crate::hardware::profiles::ProfileRegistry;
    use crate::hardware::AppConfig;
    use crate::security::license::LicenseState;
    use crate::security::users::{Role, UiProfile, User, UserStore};
    use crate::state::AppState;
    use axum_extra::extract::cookie::CookieJar;

    const TEST_USER: &str = "tester";
    const GOOD_PW: &str = "correcthorsebattery"; // ≥12 chars
    const BAD_PW: &str = "wrong-password-xyz";

    /// Build a fresh AppState with a single known user whose password is GOOD_PW.
    ///
    /// Each call uses a UNIQUE temp users-file path so `create_user`'s persisting
    /// `save()` does not leak the test user into a sibling test that reloads the
    /// same path (the shared "/nonexistent/users.json" path caused cross-test
    /// "user already exists" flakiness under parallel runs).
    async fn state_with_user() -> Arc<AppState> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let unique = format!(
            "{}-{}-{}.json",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
            "ctrl-modem-lockout-test-users"
        );
        let users_path = std::env::temp_dir().join(unique);
        let _ = std::fs::remove_file(&users_path);
        let users = UserStore::load(&users_path).await;
        let registry = ProfileRegistry::load();
        let state = Arc::new(AppState::new(
            AppConfig::default(),
            users,
            registry,
            "test-device-token".to_string(),
            Arc::new(crate::security::device_auth::DeviceAuth::ephemeral()),
            LicenseState::Unlicensed,
        ));

        let hash = hash_password(GOOD_PW).expect("hash");
        state
            .users
            .create_user(User {
                username: TEST_USER.to_string(),
                role: Role::ReadOnly,
                password_hash: Some(hash),
                allowed_panels: None,
                allowed_features: None,
                ui_profile: UiProfile::default(),
                disabled: false,
            })
            .await
            .expect("create user");
        state
    }

    fn req(username: &str, password: &str) -> LoginRequest {
        LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
        }
    }

    /// Drive `login` directly (no router). `info: None` skips the per-IP limiter.
    async fn try_login(
        state: &Arc<AppState>,
        username: &str,
        password: &str,
    ) -> ApiResult<(CookieJar, Json<LoginResponse>)> {
        login(
            State(state.clone()),
            None,
            CookieJar::new(),
            Json(req(username, password)),
        )
        .await
    }

    /// FREE_ATTEMPTS is 5 in the security module; > that many failures locks.
    const FAILURES_TO_LOCK: usize = 6;

    #[tokio::test]
    async fn failures_accumulate_then_lock_rejects_without_password_check() {
        let state = state_with_user().await;

        // Accumulate enough consecutive failures to trip the lockout.
        for _ in 0..FAILURES_TO_LOCK {
            assert!(try_login(&state, TEST_USER, BAD_PW).await.is_err());
        }

        // Precondition: the account is now locked at the lockout layer.
        assert!(
            state.login_lockout.check_locked(TEST_USER).await.is_some(),
            "account should be locked after repeated failures"
        );

        // Now even the CORRECT password is rejected — the handler must bail at
        // the lockout check BEFORE verifying credentials. A 429 (rate_limited)
        // proves it was the lockout, not a credential failure (which is 401).
        let err = match try_login(&state, TEST_USER, GOOD_PW).await {
            Err(e) => e,
            Ok(_) => panic!("locked account must be rejected even with the right password"),
        };
        assert_eq!(err.status, axum::http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(err.code, "RATE_LIMITED");
    }

    #[tokio::test]
    async fn success_clears_the_counter() {
        let state = state_with_user().await;

        // A few failures, but below the lock threshold.
        for _ in 0..3 {
            assert!(try_login(&state, TEST_USER, BAD_PW).await.is_err());
        }

        // A successful login clears the accumulated counter.
        assert!(try_login(&state, TEST_USER, GOOD_PW).await.is_ok());
        assert!(
            state.login_lockout.check_locked(TEST_USER).await.is_none(),
            "successful login must leave the account unlocked"
        );

        // And the failure budget is reset: another full sub-threshold burst of
        // failures still does not lock (proving the counter reset to zero).
        for _ in 0..3 {
            assert!(try_login(&state, TEST_USER, BAD_PW).await.is_err());
            assert!(
                state.login_lockout.check_locked(TEST_USER).await.is_none(),
                "post-success failures should not immediately re-lock"
            );
        }
    }

    #[tokio::test]
    async fn lockout_is_temporary_not_permanent() {
        // The lockout must be self-clearing (root recovery guarantee). We assert
        // the lock has a *finite* remaining duration — never permanence.
        let state = state_with_user().await;
        for _ in 0..FAILURES_TO_LOCK {
            let _ = try_login(&state, TEST_USER, BAD_PW).await;
        }
        let remaining = state
            .login_lockout
            .check_locked(TEST_USER)
            .await
            .expect("locked");
        assert!(
            remaining > std::time::Duration::ZERO,
            "lock must be active now"
        );
        // Bounded: the security module caps backoff at a few minutes. Assert it
        // is comfortably finite (not asserting the exact cap, just non-permanence).
        assert!(
            remaining <= std::time::Duration::from_secs(600),
            "lock must be temporary (bounded backoff), never permanent"
        );
    }

    #[tokio::test]
    async fn unrelated_account_is_unaffected() {
        let state = state_with_user().await;
        for _ in 0..FAILURES_TO_LOCK {
            let _ = try_login(&state, TEST_USER, BAD_PW).await;
        }
        assert!(state.login_lockout.check_locked(TEST_USER).await.is_some());
        // A different username is not locked by another account's failures.
        assert!(
            state.login_lockout.check_locked("someone-else").await.is_none(),
            "lockout must be per-account"
        );
    }
}
