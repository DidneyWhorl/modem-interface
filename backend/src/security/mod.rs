//! Security utilities.

pub mod at_whitelist;
pub mod audit;
pub mod device_auth;
pub mod license;
pub mod login_lockout;
pub mod rate_limit;
pub mod sessions;
pub mod shadow;
pub mod users;
pub mod ws_tokens;

pub use at_whitelist::*;
pub use audit::AuditLog;
// Re-exported for the API auth handler to consume — wiring pending (see
// docs/PENDING-CHANGES.md "[SECURITY → BACKEND] login lockout wiring").
#[allow(unused_imports)]
pub use login_lockout::LoginLockout;
pub use rate_limit::RateLimiter;
pub use sessions::SessionStore;
pub use users::UserStore;
pub use ws_tokens::WsTokenStore;
