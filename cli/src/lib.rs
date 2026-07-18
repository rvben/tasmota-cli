//! `tasmota-cli`: the `tasmota` command-line tool built on `tasmota-core`.
//!
//! This library holds everything except argument parsing (which lives in
//! `main.rs`): the runtime context, target resolution, rendering, and the command
//! handlers. Keeping it in a library lets the integration tests drive the same
//! code paths the binary uses.

pub mod commands;
pub mod render;
pub mod schema;
pub mod target;

use std::path::PathBuf;

use tasmota_core::Credentials;

/// Rendered output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

/// The result of a command: a single rendered blob, or a multi-device blob that
/// carries its own exit code (bulk operations report per-device and never abort).
pub enum Output {
    One(String),
    Many { rendered: String, exit_code: i32 },
}

/// Everything a command handler needs at runtime.
pub struct Ctx {
    pub format: OutputFormat,
    pub timeout_secs: u64,
    pub credentials: Option<Credentials>,
    pub cache_path: PathBuf,
    pub groups_path: PathBuf,
    pub yes: bool,
    pub dry_run: bool,
}

impl Ctx {
    /// A `tasmota-core` HTTP transport configured with this context's timeout.
    pub fn transport(&self) -> tasmota_core::HttpTransport {
        tasmota_core::HttpTransport::new(std::time::Duration::from_secs(self.timeout_secs))
    }
}

/// The config directory: `$XDG_CONFIG_HOME/tasmota` or `$HOME/.config/tasmota`.
pub fn config_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_CONFIG_HOME")
        && !x.is_empty()
    {
        return PathBuf::from(x).join("tasmota");
    }
    if let Ok(h) = std::env::var("HOME")
        && !h.is_empty()
    {
        return PathBuf::from(h).join(".config").join("tasmota");
    }
    PathBuf::from(".tasmota")
}

/// Resolve credentials from explicit flags, falling back to the environment.
/// Tasmota web auth uses username `admin` by default when only a password is set.
pub fn resolve_credentials(user: Option<String>, password: Option<String>) -> Option<Credentials> {
    let user = user
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("TASMOTA_USER").ok().filter(|s| !s.is_empty()));
    let password = password.filter(|s| !s.is_empty()).or_else(|| {
        std::env::var("TASMOTA_PASSWORD")
            .ok()
            .filter(|s| !s.is_empty())
    });
    match (user, password) {
        (None, None) => None,
        (Some(u), Some(p)) => Some(Credentials {
            user: u,
            password: p,
        }),
        (Some(u), None) => Some(Credentials {
            user: u,
            password: String::new(),
        }),
        (None, Some(p)) => Some(Credentials {
            user: "admin".into(),
            password: p,
        }),
    }
}
