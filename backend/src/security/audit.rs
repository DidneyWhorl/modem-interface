//! Security audit log.
//!
//! In-memory ring buffer of security-relevant events.
//! Also logged via tracing for persistence to stdout/syslog.

use std::collections::VecDeque;
use std::net::IpAddr;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::RwLock;
use tracing::info;

const MAX_AUDIT_ENTRIES: usize = 1000;

/// Type of security event.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    LoginSuccess,
    LoginFailure,
    Logout,
    SetupComplete,
    RateLimited,
    PasswordChanged,
    UserCreated,
    UserUpdated,
    UserDeleted,
    PasswordReset,
    #[allow(dead_code)]
    AtCommand,
    ConfigChanged,
}

/// A security audit event.
#[derive(Debug, Clone, Serialize)]
pub struct AuditEvent {
    pub timestamp: DateTime<Utc>,
    pub event_type: AuditEventType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    pub details: String,
}

/// In-memory audit log with fixed-size ring buffer.
pub struct AuditLog {
    events: RwLock<VecDeque<AuditEvent>>,
}

impl AuditLog {
    pub fn new() -> Self {
        Self {
            events: RwLock::new(VecDeque::with_capacity(MAX_AUDIT_ENTRIES)),
        }
    }

    /// Record a security event.
    pub async fn log(
        &self,
        event_type: AuditEventType,
        ip: Option<IpAddr>,
        details: impl Into<String>,
    ) {
        let details = details.into();
        let ip_str = ip.map(|i| i.to_string());

        // Structured tracing for syslog/stdout persistence
        info!(
            target: "audit",
            event = ?event_type,
            ip = ?ip_str,
            "{}", details
        );

        let event = AuditEvent {
            timestamp: Utc::now(),
            event_type,
            ip: ip_str,
            details,
        };

        let mut events = self.events.write().await;
        if events.len() >= MAX_AUDIT_ENTRIES {
            events.pop_front();
        }
        events.push_back(event);
    }

    /// Get the most recent `count` events (newest first).
    pub async fn recent(&self, count: usize) -> Vec<AuditEvent> {
        let events = self.events.read().await;
        events.iter().rev().take(count).cloned().collect()
    }
}
