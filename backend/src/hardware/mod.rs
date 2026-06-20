//! Hardware abstraction layer.
//!
//! This module defines the traits and types for modem interaction. The actual
//! protocol implementations (QMI, MBIM, MHI, AT) are provided by the Hardware
//! workstream and implement the `ModemHardware` trait defined here.
//!
//! ## Module Structure
//!
//! - `types`: Shared data structures (ModemStatus, SignalInfo, etc.)
//! - `traits`: The `ModemHardware` trait and related types
//! - `profiles`: Modem profile definitions and registry
//! - `mock`: Development/testing implementation (feature-gated)
//!
//! ## Concurrency Pattern
//!
//! The API layer wraps hardware access in `Arc<tokio::sync::Mutex<Box<dyn ModemHardware>>>`.
//! All modem operations are inherently serial, so the mutex ensures exclusive access.

pub mod fingerprint;
pub mod profiles;
#[cfg_attr(not(feature = "tunnel"), allow(dead_code))]
pub mod speedtest;
pub mod traits;
pub mod types;
pub mod usbnet;

#[cfg(feature = "mock-hardware")]
pub mod mock;

// Re-export commonly used items
pub use profiles::{DualSimConfig, ModemProfile, NetworkModeOption, ProfileRegistry};
pub use traits::*;
pub use types::*;
pub use usbnet::detect_usbnet_mode_with_bus_port;

#[cfg(feature = "mock-hardware")]
pub use mock::MockHardware;
