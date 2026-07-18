//! `tasmota-core`: the I/O-agnostic core behind the `tasmota` CLI.
//!
//! It models a Tasmota device from its `Status 0` response, talks to devices over
//! HTTP, discovers them on a network, and classifies destructive commands. The CLI
//! and (later) the web platform both build on this crate; it contains no terminal
//! rendering or process-exit logic.
//!
//! Guiding rules, enforced by tests:
//! - Absent data is `None` (never coerced to `0`).
//! - Live relay state comes from `StatusSTS.POWER`, never the `Status.Power` bitmask.
//! - HTTP 200 is not success; the JSON body decides.
//! - No network addresses are hardcoded.

pub mod cache;
pub mod discovery;
mod error;
pub mod guardrail;
pub mod model;
pub mod ops;
pub mod parse;
pub mod transport;

pub use error::{Error, Result};
pub use model::{DeviceStatus, Energy, MqttInfo, NetInfo, Relay, RelayState};
pub use transport::{Credentials, DeviceAddr, HttpTransport, Transport};
