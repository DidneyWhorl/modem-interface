//! CSRF defense-in-depth: Origin/Referer check on state-changing requests.
//!
//! The primary CSRF defense is the session cookie's `SameSite=Strict` attribute
//! (set in `routes/auth.rs`). This middleware adds a second, independent layer:
//! for state-changing HTTP methods it verifies that any supplied `Origin` (or,
//! failing that, `Referer`) header is same-origin with the request `Host`.
//!
//! ## Decision table (state-changing method: POST / PUT / DELETE / PATCH)
//!
//! - Neither `Origin` nor `Referer` present  → **ALLOW**. Native / CLI clients
//!   (the `modem-interface` CLI, busybox fetchers, server-side relays) send no
//!   Origin; rejecting them would break legitimate non-browser flows.
//! - `Origin` (or fallback `Referer`) present AND its host:port matches the
//!   request `Host` header → **ALLOW** (same-origin SPA).
//! - Otherwise (a present, cross-origin Origin/Referer) → **REJECT 403**.
//!
//! Safe (non-state-changing) methods — GET / HEAD / OPTIONS — are never checked,
//! so a cross-origin `Origin` on a GET passes through untouched.
//!
//! ## Scope / tunnel caveat
//!
//! This layer is registered ONLY on the browser-facing protected API group (see
//! `api/mod.rs`). It is intentionally NOT applied to public routes, the
//! WebSocket upgrade, or the tunnel-internal `speedtest/run-sync` path. The
//! portal reaches the router through the WSS tunnel relay, which forwards
//! requests to `127.0.0.1` carrying the portal's original `Host` (the tunnel
//! subdomain). A browser-originated cross-origin `Origin` on such a relayed
//! mutation WOULD be rejected here; legitimate portal mutations are relayed
//! server-side (no `Origin` header) and therefore pass. This must be
//! bench-verified against the portal-through-tunnel mutation path before any
//! stable cut.

use std::net::SocketAddr;

use axum::{
    extract::{connect_info::ConnectInfo, Request},
    http::{header, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};

use crate::api::error::ApiErrorResponse;

/// Returns true for HTTP methods that mutate state and therefore warrant a CSRF
/// origin check.
fn is_state_changing(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::DELETE | Method::PATCH
    )
}

/// Extract the `host:port` authority from an Origin/Referer header value.
///
/// Accepts either a bare authority (`example.com:8443`) or a full URL
/// (`https://example.com:8443/path`). Returns the lowercased authority with any
/// scheme, path, query, fragment, and userinfo stripped. Returns `None` for the
/// opaque `Origin: null` value (treated as cross-origin → caller rejects).
fn authority_of(value: &str) -> Option<String> {
    let v = value.trim();
    if v.is_empty() || v.eq_ignore_ascii_case("null") {
        return None;
    }
    // Strip scheme (everything up to and including "://").
    let after_scheme = match v.find("://") {
        Some(idx) => &v[idx + 3..],
        None => v,
    };
    // Authority ends at the first '/', '?', or '#'.
    let authority_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    let authority = &after_scheme[..authority_end];
    // Drop any userinfo ("user@host").
    let host_port = match authority.rfind('@') {
        Some(idx) => &authority[idx + 1..],
        None => authority,
    };
    if host_port.is_empty() {
        return None;
    }
    Some(host_port.to_ascii_lowercase())
}

/// Select the request's effective authority (`host:port`) for the same-origin
/// comparison. Prefers the HTTP/1.1 `Host` header when present; otherwise falls
/// back to the URI authority, which carries the HTTP/2 `:authority` pseudo-header
/// (hyper/axum exposes it via `request.uri().authority()`, never via the `Host`
/// header on h2 requests). Returns `None` only when both are absent. Pure function —
/// unit-tested below.
fn effective_host<'a>(
    host_header: Option<&'a str>,
    uri_authority: Option<&'a str>,
) -> Option<&'a str> {
    host_header.or(uri_authority)
}

/// Decide whether a state-changing request should be allowed, given the relevant
/// header values and whether the request's source IP is loopback. Pure function —
/// unit-tested below.
///
/// `peer_is_loopback` is `true` when the request arrived from a loopback source
/// address (`127.0.0.0/8` or `::1`). A loopback source is unforgeable from a LAN
/// client (the kernel drops martian-source packets on physical interfaces), so it
/// reliably identifies an on-device or portal-through-tunnel-relayed request. Such
/// requests are exempt from the cross-origin reject: a browser-driven CSRF attack
/// always originates from the victim's browser on a routable IP, never loopback.
///
/// `host` is the request `Host` header; `origin`/`referer` are the corresponding
/// request headers (already converted to `&str`). Returns `true` to ALLOW.
fn csrf_allows(
    peer_is_loopback: bool,
    host: Option<&str>,
    origin: Option<&str>,
    referer: Option<&str>,
) -> bool {
    // Loopback source → on-device / portal-through-tunnel relay → ALLOW,
    // regardless of any relayed cross-origin Origin/Referer.
    if peer_is_loopback {
        return true;
    }

    // Prefer Origin; fall back to Referer when Origin is absent.
    let source = origin.or(referer);

    let Some(source) = source else {
        // Neither header present → native/CLI/server-relay client → ALLOW.
        return true;
    };

    let source_authority = match authority_of(source) {
        Some(a) => a,
        // Present but unparsable/opaque (e.g. "null") → treat as cross-origin.
        None => return false,
    };

    // No Host to compare against → cannot prove same-origin; reject to be safe.
    let Some(host) = host.map(str::trim).filter(|h| !h.is_empty()) else {
        return false;
    };

    source_authority == host.to_ascii_lowercase()
}

/// Axum middleware enforcing the CSRF Origin/Referer check on state-changing
/// requests. Registered on the protected API group in `api/mod.rs`.
pub async fn csrf_middleware(
    peer: Option<ConnectInfo<SocketAddr>>,
    request: Request,
    next: Next,
) -> Response {
    if !is_state_changing(request.method()) {
        return next.run(request).await;
    }

    let peer_is_loopback = peer.map(|ci| ci.0.ip().is_loopback()).unwrap_or(false);

    // HTTP/2 carries the authority in the `:authority` pseudo-header (exposed via
    // the request URI), NOT in a `Host` header. Capture it before borrowing
    // headers so we can fall back to it when the `Host` header is absent.
    let uri_authority = request.uri().authority().map(|a| a.as_str());

    let headers = request.headers();
    let host_header = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let referer = headers.get(header::REFERER).and_then(|v| v.to_str().ok());

    // HTTP/2-aware host: prefer the `Host` header, fall back to the URI authority.
    let host = effective_host(host_header, uri_authority);

    if csrf_allows(peer_is_loopback, host, origin, referer) {
        return next.run(request).await;
    }

    let body = ApiErrorResponse {
        message: "Cross-origin request rejected".to_string(),
        code: "CSRF_REJECTED".to_string(),
        details: None,
    };
    (StatusCode::FORBIDDEN, Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_origin_no_referer_allows() {
        // CLI / busybox / server-side relay: no browser headers → allow.
        assert!(csrf_allows(false, Some("router.local:8443"), None, None));
    }

    #[test]
    fn same_origin_allows() {
        assert!(csrf_allows(
            false,
            Some("router.local:8443"),
            Some("https://router.local:8443"),
            None
        ));
    }

    #[test]
    fn same_origin_case_insensitive() {
        assert!(csrf_allows(
            false,
            Some("Router.Local:8443"),
            Some("https://router.local:8443"),
            None
        ));
    }

    #[test]
    fn cross_origin_rejected() {
        assert!(!csrf_allows(
            false,
            Some("router.local:8443"),
            Some("https://evil.example.com"),
            None
        ));
    }

    #[test]
    fn cross_origin_different_port_rejected() {
        assert!(!csrf_allows(
            false,
            Some("router.local:8443"),
            Some("https://router.local:9999"),
            None
        ));
    }

    #[test]
    fn referer_fallback_same_origin_allows() {
        // No Origin, but a same-origin Referer → allow.
        assert!(csrf_allows(
            false,
            Some("router.local:8443"),
            None,
            Some("https://router.local:8443/ctrl-modem/")
        ));
    }

    #[test]
    fn referer_fallback_cross_origin_rejected() {
        assert!(!csrf_allows(
            false,
            Some("router.local:8443"),
            None,
            Some("https://evil.example.com/page")
        ));
    }

    #[test]
    fn origin_preferred_over_referer() {
        // Cross-origin Origin must reject even if Referer is same-origin.
        assert!(!csrf_allows(
            false,
            Some("router.local:8443"),
            Some("https://evil.example.com"),
            Some("https://router.local:8443/")
        ));
    }

    #[test]
    fn opaque_null_origin_rejected() {
        assert!(!csrf_allows(
            false,
            Some("router.local:8443"),
            Some("null"),
            None
        ));
    }

    #[test]
    fn missing_host_with_origin_rejected() {
        // Can't prove same-origin without a Host → reject.
        assert!(!csrf_allows(
            false,
            None,
            Some("https://router.local:8443"),
            None
        ));
    }

    #[test]
    fn bare_authority_origin_allows() {
        // Defensive: Origin given as a bare authority (no scheme).
        assert!(csrf_allows(
            false,
            Some("router.local:8443"),
            Some("router.local:8443"),
            None
        ));
    }

    // --- Loopback exemption (gate (a)): a loopback source is the unforgeable
    // "on-device / portal-through-tunnel relay" signal and is exempt from the
    // cross-origin reject regardless of the relayed Origin/Referer. ---

    #[test]
    fn loopback_cross_origin_allows() {
        // Cross-origin Origin that WOULD be rejected from a routable peer is
        // allowed when the source IP is loopback (portal-through-tunnel relay).
        assert!(csrf_allows(
            true,
            Some("router.local:8443"),
            Some("https://portal.ctrl-modem.com"),
            None
        ));
    }

    #[test]
    fn loopback_opaque_null_origin_allows() {
        // Even an opaque `Origin: null` passes from a loopback peer.
        assert!(csrf_allows(true, Some("router.local:8443"), Some("null"), None));
    }

    #[test]
    fn loopback_missing_host_allows() {
        // Loopback exemption applies before the Host check.
        assert!(csrf_allows(
            true,
            None,
            Some("https://portal.ctrl-modem.com"),
            None
        ));
    }

    #[test]
    fn non_loopback_cross_origin_still_rejected() {
        // Preserved behavior: same cross-origin POST from a non-loopback peer
        // is still rejected.
        assert!(!csrf_allows(
            false,
            Some("router.local:8443"),
            Some("https://portal.ctrl-modem.com"),
            None
        ));
    }

    // --- HTTP/2 `:authority` fallback (effective_host). Under h2 there is no
    // `Host` header; the authority is carried in the URI. The middleware must use
    // the URI authority as the effective host so a same-origin h2 request matches
    // its Origin exactly as an HTTP/1.1 one does. ---

    #[test]
    fn effective_host_prefers_host_header() {
        // When both are present, the Host header wins (HTTP/1.1 behavior).
        assert_eq!(
            effective_host(Some("router.local:8443"), Some("uri.example:9999")),
            Some("router.local:8443")
        );
    }

    #[test]
    fn effective_host_falls_back_to_uri_authority() {
        // HTTP/2: no Host header → use the URI authority (`:authority`).
        assert_eq!(
            effective_host(None, Some("router.local:8443")),
            Some("router.local:8443")
        );
    }

    #[test]
    fn effective_host_none_when_both_absent() {
        assert_eq!(effective_host(None, None), None);
    }

    #[test]
    fn http2_same_origin_via_uri_authority_allows() {
        // Regression: browser over HTTP/2 sends no Host header; the same-origin
        // authority arrives via the URI (`:authority`). Feeding the effective host
        // into csrf_allows must ALLOW the same-origin mutation.
        let host = effective_host(None, Some("192.168.1.1:8443"));
        assert!(csrf_allows(
            false,
            host,
            Some("https://192.168.1.1:8443"),
            None
        ));
    }

    #[test]
    fn http2_cross_origin_via_uri_authority_rejected() {
        // HTTP/2 with no Host header but a cross-origin Origin must still REJECT.
        let host = effective_host(None, Some("192.168.1.1:8443"));
        assert!(!csrf_allows(
            false,
            host,
            Some("https://evil.example.com"),
            None
        ));
    }

    #[test]
    fn http2_cross_origin_different_port_via_uri_authority_rejected() {
        let host = effective_host(None, Some("192.168.1.1:8443"));
        assert!(!csrf_allows(
            false,
            host,
            Some("https://192.168.1.1:9999"),
            None
        ));
    }

    #[test]
    fn http1_host_header_still_used_when_present() {
        // HTTP/1.1 unchanged: Host header present is used and wins over any URI
        // authority, preserving same-origin ALLOW.
        let host = effective_host(Some("router.local:8443"), Some("ignored:1"));
        assert!(csrf_allows(
            false,
            host,
            Some("https://router.local:8443"),
            None
        ));
    }

    #[test]
    fn authority_of_strips_scheme_path_userinfo() {
        assert_eq!(
            authority_of("https://user@host.example:443/some/path?q=1#frag"),
            Some("host.example:443".to_string())
        );
        assert_eq!(authority_of("null"), None);
        assert_eq!(authority_of(""), None);
    }
}
