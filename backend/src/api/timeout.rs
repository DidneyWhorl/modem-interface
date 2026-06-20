//! Timeout utilities for hardware operations.
//!
//! Wraps hardware calls with appropriate timeouts and converts timeout
//! errors to HTTP 503 Service Unavailable with Retry-After header.

use std::future::Future;
use std::time::Duration;

use tokio::time::timeout;

use crate::api::error::ApiError;
use crate::hardware::HardwareError;

/// Operation timeout classes.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum TimeoutClass {
    /// Quick queries: 5 seconds
    /// signal, status, device_info, sim_status, detect
    Quick,
    /// State changes: 15 seconds
    /// connect, disconnect, pin operations, network select
    StateChange,
    /// Long operations: 60 seconds
    /// network_scan
    Long,
}

impl TimeoutClass {
    /// Get the duration for this timeout class.
    pub fn duration(self) -> Duration {
        match self {
            TimeoutClass::Quick => Duration::from_secs(5),
            TimeoutClass::StateChange => Duration::from_secs(15),
            TimeoutClass::Long => Duration::from_secs(60),
        }
    }

    /// Get the Retry-After value in seconds.
    pub fn retry_after(self) -> u32 {
        match self {
            TimeoutClass::Quick => 1,
            TimeoutClass::StateChange => 5,
            TimeoutClass::Long => 30,
        }
    }
}

/// Execute a hardware operation with timeout.
///
/// Returns `ApiError` on timeout or if the operation fails.
#[allow(dead_code)]
pub async fn with_timeout<T, F, Fut>(class: TimeoutClass, f: F) -> Result<T, ApiError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, HardwareError>>,
{
    match timeout(class.duration(), f()).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(hw_err)) => Err(hw_err.into()),
        Err(_elapsed) => Err(ApiError::service_unavailable_with_retry(
            "Hardware operation timed out",
            class.retry_after(),
        )),
    }
}
