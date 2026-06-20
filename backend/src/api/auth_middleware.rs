//! Authentication middleware.
//!
//! Validates session tokens from cookies on protected routes.
//! Injects SessionUser into request extensions for downstream handlers.
//! Passes through only if auth is disabled; otherwise requires a valid session.

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use axum_extra::extract::CookieJar;
use std::sync::Arc;

use crate::api::error::ApiErrorResponse;
use crate::api::routes::auth::SESSION_COOKIE;
use crate::security::users::Role;
use crate::state::AppState;

/// User identity extracted from session, available to handlers via request extensions.
#[derive(Debug, Clone)]
pub struct SessionUser {
    pub username: String,
    pub role: Role,
}

impl SessionUser {
    /// True when this session has at least Admin role (Admin or SuperAdmin).
    ///
    /// Control / state-changing routes gate on this. Read-only status/signal
    /// endpoints do NOT — see api/mod.rs route registration.
    pub fn is_admin(&self) -> bool {
        self.role >= Role::Admin
    }
}

/// Server-side authorization gate for state-changing / control routes.
///
/// Returns `Err(ApiError::forbidden(..))` (HTTP 403) when the session is below
/// Admin. Mirrors the `require_whitelist_access` pattern but without the
/// per-feature check — use this as the baseline write gate on WAN/SIM/modem/
/// tunnel/telemetry/system control handlers. GET/read routes stay ungated.
pub fn require_admin(session_user: &SessionUser) -> Result<(), crate::api::error::ApiError> {
    if session_user.is_admin() {
        Ok(())
    } else {
        Err(crate::api::error::ApiError::forbidden("Admin access required"))
    }
}

/// Middleware that validates session tokens on protected routes.
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    mut request: Request,
    next: Next,
) -> Response {
    // Check if auth is enabled
    let auth_enabled = {
        let config = state.config.read().await;
        config.auth.enabled
    };

    if !auth_enabled {
        return next.run(request).await;
    }

    // Validate session cookie
    let session_info = if let Some(cookie) = jar.get(SESSION_COOKIE) {
        state.sessions.validate(cookie.value()).await
    } else {
        None
    };

    if let Some(info) = session_info {
        // Inject user identity into request extensions
        request.extensions_mut().insert(SessionUser {
            username: info.username,
            role: info.role,
        });
        next.run(request).await
    } else {
        let body = ApiErrorResponse {
            message: "Authentication required".to_string(),
            code: "UNAUTHORIZED".to_string(),
            details: None,
        };
        (StatusCode::UNAUTHORIZED, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::AppConfig;
    use crate::hardware::profiles::ProfileRegistry;
    use crate::security::license::LicenseState;
    use crate::security::users::{User, UserStore};
    use axum::{
        body::Body,
        http::{Request as HttpRequest, StatusCode as HttpStatusCode},
        routing::get,
        Router,
    };
    use tower::ServiceExt; // for `oneshot`

    async fn protected_ok() -> &'static str {
        "ok"
    }

    /// Build an `AppState` mirroring the router-construction test pattern in
    /// `api/mod.rs`: empty user store (the path does not exist), default
    /// `AppConfig` (auth.enabled = true), no hardware, no background tasks.
    async fn build_state(config: AppConfig) -> Arc<AppState> {
        let users = UserStore::load("/nonexistent/users.json").await;
        let registry = ProfileRegistry::load();
        let device_token = "test-device-token".to_string();
        let license_state = LicenseState::Unlicensed;

        Arc::new(AppState::new(
            config,
            users,
            registry,
            device_token,
            Arc::new(crate::security::device_auth::DeviceAuth::ephemeral()),
            license_state,
        ))
    }

    /// Minimal app: the real `auth_middleware` as a `route_layer` over a dummy
    /// protected handler.
    fn app(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/protected", get(protected_ok))
            .route_layer(axum::middleware::from_fn_with_state(
                state.clone(),
                auth_middleware,
            ))
            .with_state(state)
    }

    /// Regression guard: with auth enabled and ZERO users, a protected route
    /// with no session must now be rejected (the old `!has_users` passthrough
    /// returned 200 — the pre-auth hole this change closes).
    #[tokio::test]
    async fn protected_route_rejected_when_no_users_and_no_session() {
        let state = build_state(AppConfig::default()).await;
        assert!(!state.users.has_users().await, "precondition: zero users");

        let response = app(state)
            .oneshot(
                HttpRequest::builder()
                    .uri("/protected")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), HttpStatusCode::UNAUTHORIZED);
    }

    /// A valid session reaches the protected handler even with zero stored
    /// users (root authenticates via /etc/shadow → SuperAdmin session).
    #[tokio::test]
    async fn protected_route_allowed_with_valid_session() {
        let state = build_state(AppConfig::default()).await;
        let token = state
            .sessions
            .create("root".to_string(), Role::SuperAdmin)
            .await;

        let response = app(state)
            .oneshot(
                HttpRequest::builder()
                    .uri("/protected")
                    .header("cookie", format!("{SESSION_COOKIE}={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), HttpStatusCode::OK);
    }

    /// Unchanged-path guard: users exist, no session → 401.
    #[tokio::test]
    async fn protected_route_rejected_with_no_session_when_users_exist() {
        let state = build_state(AppConfig::default()).await;
        state
            .users
            .create_user_unchecked(User {
                username: "admin".to_string(),
                role: Role::Admin,
                password_hash: Some("hash".to_string()),
                allowed_panels: None,
                allowed_features: None,
                ui_profile: Default::default(),
                disabled: false,
            })
            .await;
        assert!(state.users.has_users().await, "precondition: a user exists");

        let response = app(state)
            .oneshot(
                HttpRequest::builder()
                    .uri("/protected")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), HttpStatusCode::UNAUTHORIZED);
    }

    /// Untouched opt-out guard: auth globally disabled → passthrough even with
    /// no session and no users.
    #[tokio::test]
    async fn passthrough_when_auth_disabled() {
        let mut config = AppConfig::default();
        config.auth.enabled = false;
        let state = build_state(config).await;

        let response = app(state)
            .oneshot(
                HttpRequest::builder()
                    .uri("/protected")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), HttpStatusCode::OK);
    }
}
