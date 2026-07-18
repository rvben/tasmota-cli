//! Error type, the stable error `kind` set, and the exit-code contract.
//!
//! `tasmota-core::Error` is shared by both crates. Errors are reported by the CLI
//! as a clispec structured envelope on the last line of stderr:
//! `{"error":{"kind":...,"message":...,"exit_code":...,"hint":...}}`.
//!
//! Every kind here must be declared in the CLI's `schema.rs` `errors` set.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    /// Invalid command-line arguments (also wraps clap errors).
    #[error("{message}")]
    Usage { message: String },

    /// A device name was not found in the cache.
    #[error("{message}")]
    NotFound { message: String },

    /// The device required authentication we did not have or that was rejected.
    #[error("{message}")]
    Auth { message: String },

    /// The device was unreachable, timed out, or the transport failed.
    #[error("{message}")]
    Network { message: String },

    /// The device accepted the HTTP request (200) but rejected the command in the
    /// JSON body (e.g. `{"Command":"Unknown"}`). HTTP status never means success.
    #[error("device rejected command `{command}`: {message}")]
    CommandRejected { command: String, message: String },

    /// A device response did not have the shape we require to answer the query.
    #[error("{message}")]
    Parse { message: String },

    /// A local file operation failed (device cache, backup file).
    #[error("{message}")]
    Io { message: String },

    /// The requested datum is genuinely not available from the device. Distinct
    /// from a zero reading: never coerced to `0`.
    #[error("{message}")]
    Unavailable { message: String },

    /// The user declined a confirmation prompt for a destructive operation.
    #[error("{message}")]
    Aborted { message: String },
}

impl Error {
    /// Stable snake_case identifier consumers branch on (the schema `errors` set).
    pub fn kind(&self) -> &'static str {
        match self {
            Error::Usage { .. } => "usage",
            Error::NotFound { .. } => "not_found",
            Error::Auth { .. } => "auth",
            Error::Network { .. } => "network",
            Error::CommandRejected { .. } => "command_rejected",
            Error::Parse { .. } => "parse",
            Error::Io { .. } => "io",
            Error::Unavailable { .. } => "unavailable",
            Error::Aborted { .. } => "aborted",
        }
    }

    /// Actionable remediation, when there is one.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Error::Usage { .. } => Some("see `tasmota --help` or `tasmota schema`"),
            Error::NotFound { .. } => {
                Some("run `tasmota discover` first, or target with `--host <ip>`")
            }
            Error::Auth { .. } => {
                Some("pass `--user`/`--password`, or set TASMOTA_USER / TASMOTA_PASSWORD")
            }
            Error::Network { .. } => Some("check the device is powered and on the network"),
            _ => None,
        }
    }

    /// The process exit code associated with this error.
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Io { .. } => 1,
            Error::Aborted { .. } => 2,
            Error::Usage { .. } => 3,
            Error::NotFound { .. } => 4,
            Error::Auth { .. } => 5,
            Error::Network { .. } => 6,
            Error::CommandRejected { .. } => 7,
            Error::Parse { .. } => 8,
            Error::Unavailable { .. } => 9,
        }
    }
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, Error>;
