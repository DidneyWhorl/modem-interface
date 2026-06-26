//! API layer - HTTP and WebSocket handlers.
//!
//! This module assembles the Axum router with all routes and middleware.
//! Routes are split into public (no auth) and protected (auth required) groups.

pub mod auth_middleware;
pub mod csrf_middleware;
pub mod embedded_assets;
pub mod error;
pub mod heartbeat;
pub mod telemetry;
pub mod rate_limit_middleware;
pub mod routing;
pub mod routes;
pub mod steering;
pub mod timeout;
pub mod tunnel;
pub mod websocket;

use axum::{
    middleware,
    routing::{get, post, put},
    Router,
};
use std::sync::Arc;
use tower_http::{compression::CompressionLayer, trace::TraceLayer};

use crate::state::AppState;

/// Build the complete API router.
///
/// Routes are split into public and protected groups:
/// - **Public:** auth endpoints, health check, static files
/// - **Protected:** all modem/SIM/network/config/update endpoints + WebSocket
///
/// CORS is intentionally absent — the frontend is served from the same origin,
/// so cross-origin requests are rejected by default (no Access-Control-Allow-Origin header).
pub fn router(state: Arc<AppState>) -> Router {
    // Public routes (no auth required)
    let public_routes = Router::new()
        // Auth endpoints
        .route("/auth/login", post(routes::auth::login))
        .route("/auth/logout", post(routes::auth::logout))
        .route("/auth/status", get(routes::auth::status))
        .route("/auth/setup", post(routes::auth::setup))
        // License endpoints (always accessible)
        .route("/license/status", get(routes::license::get_license_status))
        .route("/license/activate", post(routes::license::activate_license))
        // Modem signal/status (shown on login page) - now per-modem
        .route("/modem/:modem_id/signal", get(routes::modem::public_signal))
        .route("/modem/:modem_id/status", get(routes::modem::public_status))
        // Speedtest sync endpoint (no auth — called by portal through tunnel)
        .route("/speedtest/run-sync", post(routes::speedtest::run_speedtest_sync));

    // Protected API routes (require auth)
    let protected_api = Router::new()
        // Auth actions (require active session)
        .route(
            "/auth/change-password",
            post(routes::auth::change_password),
        )
        .route("/auth/ws-token", post(routes::auth::ws_token))
        // User management routes (admin+)
        .route("/users", get(routes::users::list_users))
        .route("/users", post(routes::users::create_user))
        .route("/users/:username", get(routes::users::get_user))
        .route("/users/:username", put(routes::users::update_user))
        .route("/users/:username", axum::routing::delete(routes::users::delete_user))
        .route(
            "/users/:username/reset-password",
            post(routes::users::reset_password),
        )
        // Profile routes (any authenticated user)
        .route("/profile", get(routes::profile::get_profile))
        .route("/profile", put(routes::profile::update_profile))
        // Full license detail (tier/expiry/user_id) — authenticated only.
        // Public `/license/status` (above) returns the reduced shape (L-01).
        .route("/license/detail", get(routes::license::get_license_detail))
        // Multi-modem routes (list all modems, global operations)
        .route("/modems", get(routes::modem::list_modems))
        .route("/modem/profiles", get(routes::modem_profiles::list_profiles))
        .route("/modem/detected", get(routes::modem_profiles::detected_modems))
        .route("/modem/rescan", post(routes::modem_profiles::rescan_modems))
        // Backward-compat routes (old single-modem API, uses selected modem)
        .route("/modem/select", post(routes::modem::select_modem_compat))
        .route("/modem/status", get(routes::modem::status_compat))
        .route("/modem/signal", get(routes::modem::signal_compat))
        .route("/modem/info", get(routes::modem::info_compat))
        .route("/modem/gps", get(routes::modem::gps_compat))
        .route("/modem/pdp", get(routes::modem::pdp_details_compat))
        .route("/modem/signal/antenna", get(routes::modem::antenna_metrics_compat))
        .route("/modem/signal/extended", get(routes::modem::signal_extended_compat))
        .route("/modem/signal/history", get(routes::modem::signal_history_compat))
        .route("/modem/connect", post(routes::modem::connect_compat))
        .route("/modem/disconnect", post(routes::modem::disconnect_compat))
        .route("/modem/reconnect", post(routes::modem::reconnect_compat))
        .route("/sim/status", get(routes::modem::sim_status_compat))
        .route("/config", get(routes::modem::get_config_compat))
        .route("/modem/command", post(routes::modem::command_compat))
        .route("/modem/profile/active", get(routes::modem_profiles::active_profile_compat))
        // Single-modem routes (signal & status are public, see above)
        .route("/modem/:modem_id/detect", get(routes::modem::detect))
        .route("/modem/:modem_id/info", get(routes::modem::info))
        .route("/modem/:modem_id/stats", get(routes::modem::stats))
        .route("/modem/:modem_id/connect", post(routes::modem::connect))
        .route("/modem/:modem_id/disconnect", post(routes::modem::disconnect))
        .route("/modem/:modem_id/reconnect", post(routes::modem::reconnect))
        .route("/modem/:modem_id/command", post(routes::modem::command))
        .route("/modem/:modem_id/gps", get(routes::modem::gps))
        .route("/modem/:modem_id/gps/stop", post(routes::modem::gps_stop))
        .route("/modem/:modem_id/pdp", get(routes::modem::pdp_details))
        .route("/modem/:modem_id/signal/extended", get(routes::modem::signal_extended))
        .route("/modem/:modem_id/signal/antenna", get(routes::modem::antenna_metrics))
        .route("/modem/:modem_id/signal/history", get(routes::modem::signal_history))
        // On-demand refresh endpoints (bypass cache, hit hardware directly)
        .route("/modem/:modem_id/signal/refresh", post(routes::modem::signal_refresh))
        .route("/modem/:modem_id/status/refresh", post(routes::modem::status_refresh))
        .route("/modem/:modem_id/device/refresh", post(routes::modem::device_refresh))
        .route("/modem/:modem_id/sim/refresh", post(routes::modem::sim_refresh))
        .route("/modem/:modem_id/gps/refresh", post(routes::modem::gps_refresh))
        .route("/modem/:modem_id/registration/refresh", post(routes::modem::registration_refresh))
        // GPS panel gate (controls GPS polling in cache refresh task)
        .route("/gps/panel", post(routes::modem::gps_panel))
        // Modem power control routes
        .route("/modem/:modem_id/health", get(routes::modem::health))
        .route("/modem/:modem_id/power-down", post(routes::modem::power_down))
        .route("/modem/:modem_id/reboot", post(routes::modem::reboot))
        .route("/modem/:modem_id/airplane", get(routes::modem::airplane_status).post(routes::modem::airplane))
        // Band locking & mode control
        .route("/modem/:modem_id/bands", get(routes::modem::get_band_config).post(routes::modem::set_band_config))
        .route("/modem/:modem_id/bands/restore", post(routes::modem::restore_bands))
        // MBN carrier profile management
        .route("/modem/:modem_id/mbn/select", post(routes::modem::mbn_select))
        .route("/modem/:modem_id/mbn/deactivate", post(routes::modem::mbn_deactivate))
        .route("/modem/:modem_id/mbn/auto-select", post(routes::modem::mbn_auto_select))
        // Diff-aware APN apply (live write vs MBN reboot) — Item #42 Phase 2
        .route("/modem/:modem_id/apn/apply", post(routes::modem::apn_apply))
        // APN Profile management
        .route("/modem/:modem_id/apn-profiles/apply", post(routes::modem::apply_apn_profile))
        .route("/modem/:modem_id/apn-profiles/export", get(routes::modem::export_apn_profiles))
        .route("/modem/:modem_id/apn-profiles/import", post(routes::modem::import_apn_profiles))
        .route("/modem/:modem_id/apn-profiles", get(routes::modem::list_apn_profiles).post(routes::modem::create_apn_profile))
        .route("/modem/:modem_id/apn-profiles/:id", put(routes::modem::update_apn_profile).delete(routes::modem::delete_apn_profile))
        // AT whitelist management (Admin+ with at-whitelist feature)
        .route("/modem/:modem_id/whitelist", get(routes::modem::get_whitelist).put(routes::modem::update_whitelist))
        // Per-modem profile routes
        .route("/modem/:modem_id/profile", get(routes::modem_profiles::active_profile))
        .route("/modem/:modem_id/profile/override", post(routes::modem_profiles::override_profile))
        .route("/modem/:modem_id/profile/request", post(routes::modem_profiles::request_profile))
        .route("/modem/:modem_id/discover", post(routes::modem_profiles::discover_modem))
        // SIM routes (nested under modem)
        .route("/modem/:modem_id/sim/status", get(routes::sim::status))
        .route("/modem/:modem_id/sim/pin", post(routes::sim::pin))
        // Dual SIM slot management
        .route("/modem/:modem_id/sim/slots", get(routes::sim::get_sim_slots))
        .route("/modem/:modem_id/sim/slots/config", get(routes::sim::get_sim_slot_config).put(routes::sim::update_sim_slot_config))
        .route("/modem/:modem_id/sim/slots/switch", post(routes::sim::switch_sim_slot))
        // WAN manager routes
        .route("/wan/status", get(routes::wan::get_wan_status))
        .route("/wan/config", put(routes::wan::update_wan_config))
        .route("/wan/scan", post(routes::wan::scan_wan))
        .route("/wan/add-ethernet", post(routes::wan::add_ethernet))
        .route("/wan/failback", post(routes::wan::failback_now))
        .route("/wan/accept-failover", post(routes::wan::accept_failover))
        .route("/wan/watchdog/log", get(routes::wan::get_watchdog_log))
        .route("/wan/watchdog/log/clear", post(routes::wan::clear_watchdog_log_handler))
        .route("/wan/watchdog/log/download", get(routes::wan::download_watchdog_log))
        .route("/wan/watchdog/restart-suspension/clear", post(routes::wan::clear_restart_suspensions))
        // Traffic steering routes (Level 2)
        .route("/wan/steering", get(routes::steering::list_rules).post(routes::steering::create_rule))
        .route("/wan/steering/reorder", put(routes::steering::reorder_rules))
        .route("/wan/steering/:id", put(routes::steering::update_rule).delete(routes::steering::delete_rule))
        // Speedtest routes
        .route("/speedtest/run", post(routes::speedtest::run_speedtest))
        .route("/speedtest/status", get(routes::speedtest::get_status))
        .route("/speedtest/history", get(routes::speedtest::get_history))
        // Network routes (per-modem)
        .route("/modem/:modem_id/network/scan", get(routes::network::scan))
        .route("/modem/:modem_id/network/select", post(routes::network::select))
        .route("/modem/:modem_id/network/registration", get(routes::network::registration))
        // Config routes (per-modem)
        .route("/modem/:modem_id/config", get(routes::config::get_config))
        .route("/modem/:modem_id/config", put(routes::config::update_config))
        // System routes (all protected — version info requires auth)
        .route("/system/version", get(routes::system::get_version))
        .route("/system/update/check", get(routes::system::check_update))
        .route("/system/update/apply", post(routes::system::apply_update))
        .route(
            "/system/update/status",
            get(routes::system::get_update_status),
        )
        .route("/system/update/log", get(routes::system::get_update_log))
        // Telemetry config
        .route("/telemetry/config", get(telemetry::get_telemetry_config).put(telemetry::update_telemetry_config))
        // Tunnel config
        .route("/tunnel/config", get(tunnel::get_tunnel_config).put(tunnel::update_tunnel_config))
        // Telemetry polling controls
        .route("/telemetry/polling", get(telemetry::get_telemetry_polling).put(telemetry::update_telemetry_polling))
        .route("/telemetry/poll-now", post(telemetry::trigger_poll_now))
        // Audit log (protected)
        .route("/system/audit", get(routes::system::get_audit_log))
        // Apply auth middleware to protected routes only.
        //
        // NOTE: there is intentionally NO global license gate here. The
        // license/portal is optional (v1.4.0-dev.2 pivot) — unlicensed devices
        // get full LOCAL API access. Cloud features stay gated by their own
        // per-feature checks (e.g. the remote-access tunnel's
        // `has_feature("remote_access")` + `tunnel.enabled` gate in tunnel.rs).
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware::auth_middleware,
        ))
        // Apply general rate limit to protected routes
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit_middleware::rate_limit_middleware,
        ))
        // CSRF defense-in-depth: Origin/Referer check on state-changing methods.
        // Scoped to the browser-facing protected API only — NOT the public
        // routes, the WebSocket upgrade, or the tunnel-internal speedtest
        // run-sync path. See csrf_middleware.rs for the tunnel caveat.
        .route_layer(middleware::from_fn(csrf_middleware::csrf_middleware));

    // WebSocket route: auth handled in-message (token), not via cookie middleware.
    // Rate limiting still applies to the HTTP upgrade request.
    let ws_routes = Router::new()
        .route("/events", get(websocket::events_handler))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit_middleware::rate_limit_middleware,
        ));

    let api_routes = Router::new()
        .merge(public_routes)
        .merge(protected_api)
        .merge(ws_routes);

    // Frontend serving under /ctrl-modem prefix
    #[cfg(feature = "embedded-frontend")]
    let frontend_routes = {
        tracing::info!("Serving embedded frontend assets under /ctrl-modem/");
        Router::new().fallback(embedded_assets::serve_embedded)
    };

    #[cfg(not(feature = "embedded-frontend"))]
    let frontend_routes = {
        use tower_http::services::{ServeDir, ServeFile};
        let static_path = std::env::var("STATIC_FILES_PATH")
            .unwrap_or_else(|_| "/www/modem-interface".to_string());
        tracing::info!("Serving static files from: {}", static_path);
        let index_path = format!("{static_path}/index.html");
        let serve_dir =
            ServeDir::new(&static_path).not_found_service(ServeFile::new(&index_path));
        Router::new().fallback_service(serve_dir)
    };

    // Nest API routes and frontend under /ctrl-modem
    let ctrl_modem = Router::new()
        .nest("/api", api_routes)
        .merge(frontend_routes);

    // Trailing-slash route: /ctrl-modem/ doesn't match nest("/ctrl-modem") fallback
    // in Axum 0.7, so add an explicit redirect.
    #[cfg(feature = "embedded-frontend")]
    let trailing_slash_route = Router::new()
        .route("/ctrl-modem/", get(embedded_assets::serve_embedded));
    #[cfg(not(feature = "embedded-frontend"))]
    let trailing_slash_route = Router::new();

    Router::new()
        .merge(trailing_slash_route)
        .nest("/ctrl-modem", ctrl_modem)
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Health check endpoint (outside /api for load balancers).
pub async fn health() -> &'static str {
    "ok"
}

/// Emit a one-time startup warning when authentication is enabled but TLS is
/// not active.
///
/// The session cookie's `Secure` flag is set to the runtime TLS state
/// (`auth.rs` login handler reads `state.tls_active`). On an HTTP-only
/// deployment the session cookie therefore rides cleartext and could be
/// captured on the wire. This warns the operator to front the service with TLS
/// so the cookie gains the `Secure` attribute.
///
/// Call this from the startup/serve owner (`main.rs`) once TLS activation has
/// been determined (i.e. after the TLS-start attempt has either succeeded —
/// returning — or fallen back to HTTP). On the HTTP fallback path `tls_active`
/// is still `false`, which is exactly the condition this warns about.
///
/// NOTE: the call site lives in `main.rs`, which the Backend/API session does
/// NOT own. The one-line wiring is logged in `docs/PENDING-CHANGES.md`
/// ("[Backend/API → Startup owner (main.rs)] Wire the auth-without-TLS startup
/// warning"). `#[allow(dead_code)]` keeps `-D warnings` green until main calls
/// it; remove the allow once the call site lands.
#[allow(dead_code)]
pub fn warn_if_auth_without_tls(auth_enabled: bool, tls_active: bool) {
    if auth_enabled && !tls_active {
        tracing::warn!(
            "Authentication is enabled but TLS is not active: the session cookie \
             cannot be marked Secure and will be transmitted over cleartext HTTP. \
             Front the service with TLS (or a TLS-terminating reverse proxy) so the \
             session cookie gains the Secure attribute."
        );
    }
}

// =============================================================================
// Router construction tests
// =============================================================================
//
// These tests guard the bug-class found in v1.2.0-dev.9 ("Defect 4"): two
// handlers both registered `/health`, and `axum::Router::merge` panics at
// Router *construction* time when routes overlap. Before this test existed,
// `cargo test` never instantiated the full merged main router, so CI stayed
// green while every released binary crashed at boot.

#[cfg(test)]
mod router_construction_tests {
    use super::*;
    use crate::hardware::AppConfig;
    use crate::hardware::profiles::ProfileRegistry;
    use crate::security::license::LicenseState;
    use crate::security::users::UserStore;
    use crate::state::AppState;

    /// Build the full merged main router exactly as `main.rs:319-321` does.
    /// If axum finds an overlapping route between the top-level `/health`
    /// route and anything inside `api::router`, this will panic — and the
    /// test will fail loudly at CI time instead of at boot time on hardware.
    #[tokio::test]
    async fn main_router_constructs_without_panic() {
        // Cheapest possible state: empty user store (load falls back to empty
        // map when the path does not exist), default AppConfig, fresh profile
        // registry, dummy device token, Unlicensed license state. No hardware
        // init, no background tasks, no network I/O.
        let config = AppConfig::default();
        let users = UserStore::load("/nonexistent/users.json").await;
        let registry = ProfileRegistry::load();
        let device_token = "test-device-token".to_string();
        let license_state = LicenseState::Unlicensed;

        let state = Arc::new(AppState::new(
            config,
            users,
            registry,
            device_token,
            Arc::new(crate::security::device_auth::DeviceAuth::ephemeral()),
            license_state,
        ));

        // Mirror main.rs:319-321 exactly. The `.merge(router(state))` call is
        // what panics if any nested route collides with `/health`.
        let _app = axum::Router::new()
            .route("/health", axum::routing::get(health))
            .merge(router(state));
    }
}

// =============================================================================
// License-optional routing tests
// =============================================================================
//
// The license/portal is OPTIONAL (v1.4.0-dev.2 pivot, spec
// 2026-06-18-license-gate-optional-local-features-design.md). Unlicensed
// devices must receive FULL local API access — the old blunt
// `license_middleware` gate that returned 403 `license_required` on protected
// local routes is gone. These tests guard that an authenticated request to a
// protected local route is NOT blocked for lack of a license.

#[cfg(test)]
mod license_optional_tests {
    use super::*;
    use crate::hardware::profiles::ProfileRegistry;
    use crate::hardware::AppConfig;
    use crate::security::license::LicenseState;
    use crate::api::routes::auth::SESSION_COOKIE;
    use crate::security::users::Role;
    use crate::security::users::UserStore;
    use crate::state::AppState;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode as HttpStatusCode};
    use tower::ServiceExt; // for `oneshot`

    /// Cheapest possible state with an explicit license state: empty user store,
    /// default config, no hardware, no background tasks.
    async fn build_state(license_state: LicenseState) -> Arc<AppState> {
        let config = AppConfig::default();
        let users = UserStore::load("/nonexistent/users.json").await;
        let registry = ProfileRegistry::load();
        let device_token = "test-device-token".to_string();

        Arc::new(AppState::new(
            config,
            users,
            registry,
            device_token,
            Arc::new(crate::security::device_auth::DeviceAuth::ephemeral()),
            license_state,
        ))
    }

    /// Core guard for the pivot: with NO license (Unlicensed) and a valid
    /// session, a protected local route (`GET /modems`) must reach its handler
    /// — it must NOT be blocked with `403 license_required`. Auth still applies
    /// (the valid session satisfies it); only the licensing gate is gone.
    #[tokio::test]
    async fn protected_local_route_reachable_without_license() {
        let state = build_state(LicenseState::Unlicensed).await;
        let token = state
            .sessions
            .create("root".to_string(), Role::SuperAdmin)
            .await;

        let response = router(state)
            .oneshot(
                HttpRequest::builder()
                    .uri("/ctrl-modem/api/modems")
                    .header("cookie", format!("{SESSION_COOKIE}={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // The blunt license gate would have returned 403 here. With it removed,
        // the authenticated request reaches the handler (empty modem list → 200).
        assert_ne!(
            response.status(),
            HttpStatusCode::FORBIDDEN,
            "protected local route must not be license-gated when unlicensed"
        );
        assert_eq!(response.status(), HttpStatusCode::OK);
    }

    /// Without a session the same route is still rejected by AUTH (401), proving
    /// removing the license gate did not weaken authentication.
    #[tokio::test]
    async fn protected_local_route_still_requires_auth() {
        let state = build_state(LicenseState::Unlicensed).await;

        let response = router(state)
            .oneshot(
                HttpRequest::builder()
                    .uri("/ctrl-modem/api/modems")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), HttpStatusCode::UNAUTHORIZED);
    }
}

// =============================================================================
// CSRF Origin-check middleware tests (FIX 3 — defense-in-depth)
// =============================================================================
//
// Exercises the csrf_middleware layer through the full protected router. We use
// an authenticated session so the request passes auth and reaches the point
// where the CSRF layer's verdict is decisive. The CSRF layer is method-based:
// it gates POST/PUT/DELETE/PATCH and leaves GET/HEAD/OPTIONS untouched.

#[cfg(test)]
mod csrf_middleware_tests {
    use super::*;
    use crate::api::routes::auth::SESSION_COOKIE;
    use crate::hardware::profiles::ProfileRegistry;
    use crate::hardware::AppConfig;
    use crate::security::license::LicenseState;
    use crate::security::users::Role;
    use crate::security::users::UserStore;
    use crate::state::AppState;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode as HttpStatusCode};
    use tower::ServiceExt; // for `oneshot`

    async fn build_state() -> Arc<AppState> {
        let users = UserStore::load("/nonexistent/users.json").await;
        let registry = ProfileRegistry::load();
        Arc::new(AppState::new(
            AppConfig::default(),
            users,
            registry,
            "test-device-token".to_string(),
            Arc::new(crate::security::device_auth::DeviceAuth::ephemeral()),
            LicenseState::Unlicensed,
        ))
    }

    async fn session(state: &Arc<AppState>) -> String {
        state
            .sessions
            .create("root".to_string(), Role::SuperAdmin)
            .await
    }

    /// Same-origin POST (Origin host:port matches Host) passes the CSRF check
    /// and reaches the handler (not a 403 from the CSRF layer).
    #[tokio::test]
    async fn same_origin_post_passes() {
        let state = build_state().await;
        let token = session(&state).await;

        let response = router(state)
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/ctrl-modem/api/auth/ws-token")
                    .header("host", "router.local:8443")
                    .header("origin", "https://router.local:8443")
                    .header("cookie", format!("{SESSION_COOKIE}={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_ne!(
            response.status(),
            HttpStatusCode::FORBIDDEN,
            "same-origin POST must not be CSRF-rejected"
        );
    }

    /// No Origin / no Referer (CLI, busybox, server-side relay) → POST passes.
    #[tokio::test]
    async fn no_origin_post_passes() {
        let state = build_state().await;
        let token = session(&state).await;

        let response = router(state)
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/ctrl-modem/api/auth/ws-token")
                    .header("host", "router.local:8443")
                    .header("cookie", format!("{SESSION_COOKIE}={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_ne!(
            response.status(),
            HttpStatusCode::FORBIDDEN,
            "no-Origin POST (CLI/relay) must not be CSRF-rejected"
        );
    }

    /// Cross-origin POST → 403 from the CSRF layer.
    #[tokio::test]
    async fn cross_origin_post_rejected() {
        let state = build_state().await;
        let token = session(&state).await;

        let response = router(state)
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/ctrl-modem/api/auth/ws-token")
                    .header("host", "router.local:8443")
                    .header("origin", "https://evil.example.com")
                    .header("cookie", format!("{SESSION_COOKIE}={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            HttpStatusCode::FORBIDDEN,
            "cross-origin POST must be CSRF-rejected with 403"
        );
    }

    /// Loopback exemption (gate (a)): a cross-origin POST that WOULD be 403'd
    /// from a routable peer passes when the request source IP is loopback (the
    /// portal-through-tunnel relay path). We inject `ConnectInfo` into the
    /// request extensions to simulate the wired peer address.
    #[tokio::test]
    async fn cross_origin_post_from_loopback_passes() {
        use axum::extract::connect_info::ConnectInfo;
        use std::net::SocketAddr;

        let state = build_state().await;
        let token = session(&state).await;

        let mut request = HttpRequest::builder()
            .method("POST")
            .uri("/ctrl-modem/api/auth/ws-token")
            .header("host", "router.local:8443")
            .header("origin", "https://portal.ctrl-modem.com")
            .header("cookie", format!("{SESSION_COOKIE}={token}"))
            .body(Body::empty())
            .unwrap();
        let loopback: SocketAddr = "127.0.0.1:54321".parse().unwrap();
        request.extensions_mut().insert(ConnectInfo(loopback));

        let response = router(state).oneshot(request).await.unwrap();

        assert_ne!(
            response.status(),
            HttpStatusCode::FORBIDDEN,
            "cross-origin POST from a loopback peer must NOT be CSRF-rejected"
        );
    }

    /// Preserved behavior: the SAME cross-origin POST from a non-loopback peer
    /// is still 403'd.
    #[tokio::test]
    async fn cross_origin_post_from_non_loopback_rejected() {
        use axum::extract::connect_info::ConnectInfo;
        use std::net::SocketAddr;

        let state = build_state().await;
        let token = session(&state).await;

        let mut request = HttpRequest::builder()
            .method("POST")
            .uri("/ctrl-modem/api/auth/ws-token")
            .header("host", "router.local:8443")
            .header("origin", "https://portal.ctrl-modem.com")
            .header("cookie", format!("{SESSION_COOKIE}={token}"))
            .body(Body::empty())
            .unwrap();
        let lan: SocketAddr = "192.168.1.50:44444".parse().unwrap();
        request.extensions_mut().insert(ConnectInfo(lan));

        let response = router(state).oneshot(request).await.unwrap();

        assert_eq!(
            response.status(),
            HttpStatusCode::FORBIDDEN,
            "cross-origin POST from a non-loopback peer must be CSRF-rejected"
        );
    }

    /// GET is not state-changing — a cross-origin Origin must NOT be rejected.
    #[tokio::test]
    async fn cross_origin_get_passes() {
        let state = build_state().await;
        let token = session(&state).await;

        let response = router(state)
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/ctrl-modem/api/modems")
                    .header("host", "router.local:8443")
                    .header("origin", "https://evil.example.com")
                    .header("cookie", format!("{SESSION_COOKIE}={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_ne!(
            response.status(),
            HttpStatusCode::FORBIDDEN,
            "cross-origin GET (not state-changing) must pass the CSRF check"
        );
        assert_eq!(response.status(), HttpStatusCode::OK);
    }
}
