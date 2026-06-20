//! General API rate limit middleware.
//!
//! Applied to protected routes to limit per-IP request volume.
//! Returns 429 Too Many Requests with Retry-After header when exceeded.

use axum::{
    extract::{connect_info::ConnectInfo, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use std::net::SocketAddr;
use std::sync::Arc;

use crate::api::error::ApiErrorResponse;
use crate::security::audit::AuditEventType;
use crate::security::rate_limit::RateCategory;
use crate::state::AppState;

/// Middleware that enforces per-IP rate limits on API requests.
pub async fn rate_limit_middleware(
    State(state): State<Arc<AppState>>,
    info: Option<ConnectInfo<SocketAddr>>,
    request: Request,
    next: Next,
) -> Response {
    let ip = info.map(|ci| ci.0.ip());

    if let Some(ip_addr) = ip {
        if let Err(retry_after) = state.rate_limiter.check(ip_addr, RateCategory::General).await {
            let path = request.uri().path().to_string();
            state
                .audit
                .log(
                    AuditEventType::RateLimited,
                    Some(ip_addr),
                    format!("General rate limit exceeded: {path}"),
                )
                .await;

            let body = ApiErrorResponse {
                message: "Too many requests. Try again later.".to_string(),
                code: "RATE_LIMITED".to_string(),
                details: None,
            };

            let mut response = (StatusCode::TOO_MANY_REQUESTS, Json(body)).into_response();
            if let Ok(value) = retry_after.to_string().parse() {
                response
                    .headers_mut()
                    .insert(axum::http::header::RETRY_AFTER, value);
            }
            return response;
        }
    }

    next.run(request).await
}
