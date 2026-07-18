//! The local device cache: names discovered on the network mapped to hosts.
//!
//! Stored as JSON at a path the CLI resolves under XDG config. Credentials are
//! deliberately NOT persisted here (a published tool should not write plaintext
//! passwords to disk); authenticate per-invocation via flags or environment.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::DeviceStatus;

/// One remembered device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedDevice {
    pub name: String,
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
}

/// The whole cache.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceCache {
    #[serde(default)]
    pub devices: Vec<CachedDevice>,
}

impl DeviceCache {
    /// Load the cache from `path`. A missing file is an empty cache, not an error.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).map_err(|e| Error::Io {
                message: format!("device cache at {} is corrupt: {e}", path.display()),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::Io {
                message: format!("reading device cache {}: {e}", path.display()),
            }),
        }
    }

    /// Write the cache to `path`, creating parent directories as needed.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::Io {
                message: format!("creating {}: {e}", parent.display()),
            })?;
        }
        let json = serde_json::to_string_pretty(self).expect("cache serializes");
        std::fs::write(path, json).map_err(|e| Error::Io {
            message: format!("writing device cache {}: {e}", path.display()),
        })
    }

    /// Find a device by exact name (case-insensitive).
    pub fn find(&self, name: &str) -> Option<&CachedDevice> {
        self.devices
            .iter()
            .find(|d| d.name.eq_ignore_ascii_case(name))
    }

    /// Insert or update a device keyed on host.
    pub fn upsert(&mut self, device: CachedDevice) {
        if let Some(existing) = self.devices.iter_mut().find(|d| d.host == device.host) {
            *existing = device;
        } else {
            self.devices.push(device);
        }
        self.devices.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// Replace the cache contents from a fresh discovery scan.
    pub fn from_statuses(statuses: &[DeviceStatus]) -> Self {
        let mut devices: Vec<CachedDevice> = statuses
            .iter()
            .map(|s| CachedDevice {
                name: s.display_name().to_string(),
                host: s.host.clone(),
                module: s.module,
                mac: s.net.mac.clone(),
            })
            .collect();
        devices.sort_by(|a, b| a.name.cmp(&b.name));
        DeviceCache { devices }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_empty_cache() {
        let dir = std::env::temp_dir().join("tasmota-cache-test-missing");
        let path = dir.join("does-not-exist.json");
        let cache = DeviceCache::load(&path).unwrap();
        assert!(cache.devices.is_empty());
    }

    #[test]
    fn roundtrip_and_find_is_case_insensitive() {
        let mut cache = DeviceCache::default();
        cache.upsert(CachedDevice {
            name: "Dryer".into(),
            host: "192.0.2.20".into(),
            module: Some(1),
            mac: None,
        });
        assert_eq!(cache.find("dryer").unwrap().host, "192.0.2.20");
        assert!(cache.find("freezer").is_none());
    }

    #[test]
    fn upsert_replaces_on_same_host() {
        let mut cache = DeviceCache::default();
        cache.upsert(CachedDevice {
            name: "Old".into(),
            host: "192.0.2.21".into(),
            module: None,
            mac: None,
        });
        cache.upsert(CachedDevice {
            name: "New".into(),
            host: "192.0.2.21".into(),
            module: None,
            mac: None,
        });
        assert_eq!(cache.devices.len(), 1);
        assert_eq!(cache.devices[0].name, "New");
    }
}
