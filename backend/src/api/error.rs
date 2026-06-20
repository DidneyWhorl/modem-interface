//! API error handling.
//!
//! Converts hardware errors and validation failures into proper HTTP responses.

use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

use crate::hardware::HardwareError;

/// API error response body.
#[derive(Debug, Serialize)]
pub struct ApiErrorResponse {
    pub message: String,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

/// API error type that implements IntoResponse.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
    pub details: Option<String>,
    /// Retry-After value in seconds (for 503 responses)
    pub retry_after: Option<u32>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
            details: None,
            retry_after: None,
        }
    }

    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }

    pub fn with_retry_after(mut self, seconds: u32) -> Self {
        self.retry_after = Some(seconds);
        self
    }

    // Common error constructors
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "NOT_FOUND", message)
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "BAD_REQUEST", message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "FORBIDDEN", message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", message)
    }

    pub fn service_unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "UNAVAILABLE", message)
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(StatusCode::GATEWAY_TIMEOUT, "TIMEOUT", message)
    }

    pub fn service_unavailable_with_retry(message: impl Into<String>, retry_secs: u32) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "TIMEOUT", message)
            .with_retry_after(retry_secs)
    }

    #[cfg_attr(not(feature = "tunnel"), allow(dead_code))]
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "CONFLICT", message)
    }

    pub fn precondition_required(message: impl Into<String>) -> Self {
        Self::new(StatusCode::PRECONDITION_REQUIRED, "CONFIRMATION_REQUIRED", message)
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", message)
    }

    pub fn rate_limited(message: impl Into<String>, retry_after_secs: u64) -> Self {
        Self::new(StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED", message)
            .with_retry_after(retry_after_secs as u32)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ApiErrorResponse {
            message: self.message,
            code: self.code,
            details: self.details,
        };

        let mut response = (self.status, Json(body)).into_response();

        // Add Retry-After header for 503 responses
        if let Some(retry_secs) = self.retry_after {
            if let Ok(value) = HeaderValue::from_str(&retry_secs.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
        }

        response
    }
}

// Convert HardwareError to ApiError
impl From<HardwareError> for ApiError {
    fn from(err: HardwareError) -> Self {
        match err {
            HardwareError::NoModem => {
                Self::service_unavailable("No modem detected").with_details(err.to_string())
            }
            HardwareError::NotReady(msg) => {
                Self::new(StatusCode::CONFLICT, "MODEM_NOT_READY", msg)
            }
            HardwareError::DeviceNotFound(path) => {
                Self::not_found(format!("Device not found: {path}"))
            }
            HardwareError::Protocol(msg) => Self::new(
                StatusCode::BAD_GATEWAY,
                "PROTOCOL_ERROR",
                "Modem communication error",
            )
            .with_details(msg),
            HardwareError::Timeout => {
                Self::timeout("Modem operation timed out")
            }
            HardwareError::SimError(msg) => {
                Self::new(StatusCode::CONFLICT, "SIM_ERROR", msg)
            }
            HardwareError::CommandRejected(msg) => Self::bad_request(msg),
            HardwareError::PermissionDenied(msg) => Self::forbidden(msg),
            HardwareError::Io(msg) => {
                Self::internal("IO error").with_details(msg)
            }
            HardwareError::Internal(msg) => Self::internal(msg),
        }
    }
}

/// Result type for API handlers.
pub type ApiResult<T> = Result<T, ApiError>;
