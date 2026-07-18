//! Resolve the global `--host`/`--name`/`--group`/`--all` selection into a list of
//! addressable devices, plus group definitions read from `groups.toml`.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::path::Path;

use serde::Deserialize;

use tasmota_core::cache::DeviceCache;
use tasmota_core::{Credentials, DeviceAddr, Error, Result};

/// How the user asked to select devices (from global flags).
#[derive(Debug, Clone, Default)]
pub struct Selector {
    pub host: Option<String>,
    pub name: Option<String>,
    pub group: Option<String>,
    pub all: bool,
}

/// A resolved device: an address to talk to, and a human label for output.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub addr: DeviceAddr,
    pub label: String,
}

/// The `groups.toml` shape: `[groups]\nliving = ["Dryer", "192.0.2.9"]`.
#[derive(Debug, Deserialize, Default)]
struct GroupsFile {
    #[serde(default)]
    groups: BTreeMap<String, Vec<String>>,
}

fn load_groups(path: &Path) -> Result<GroupsFile> {
    match std::fs::read_to_string(path) {
        Ok(s) => toml::from_str(&s).map_err(|e| Error::Io {
            message: format!(
                "parsing {}: {e}\nexpected a [groups] table mapping each name to a list \
                 of device names or IPs, e.g.:\n\n[groups]\nkitchen = [\"Freezer\", \
                 \"Dishwasher\", \"10.10.20.5\"]",
                path.display()
            ),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(GroupsFile::default()),
        Err(e) => Err(Error::Io {
            message: format!("reading {}: {e}", path.display()),
        }),
    }
}

/// List all groups and their members.
pub fn list_groups(path: &Path) -> Result<BTreeMap<String, Vec<String>>> {
    Ok(load_groups(path)?.groups)
}

fn attach(addr_host: String, label: String, credentials: &Option<Credentials>) -> Resolved {
    Resolved {
        addr: DeviceAddr::new(addr_host).with_credentials(credentials.clone()),
        label,
    }
}

/// Resolve a member of a group, which may be a cached name or a raw IP.
fn resolve_member(
    member: &str,
    cache: &DeviceCache,
    creds: &Option<Credentials>,
) -> Result<Resolved> {
    if member.parse::<Ipv4Addr>().is_ok() {
        return Ok(attach(member.to_string(), member.to_string(), creds));
    }
    match cache.find(member) {
        Some(dev) => Ok(attach(dev.host.clone(), dev.name.clone(), creds)),
        None => Err(Error::NotFound {
            message: format!("group member `{member}` is not a cached device or an IP"),
        }),
    }
}

/// Resolve the selector into one or more devices. Exactly one selection method
/// must be given.
pub fn resolve(
    sel: &Selector,
    cache_path: &Path,
    groups_path: &Path,
    credentials: &Option<Credentials>,
) -> Result<Vec<Resolved>> {
    let methods = [
        sel.host.is_some(),
        sel.name.is_some(),
        sel.group.is_some(),
        sel.all,
    ]
    .iter()
    .filter(|b| **b)
    .count();

    if methods == 0 {
        return Err(Error::Usage {
            message: "no target selected; use --host, --name, --group, or --all".into(),
        });
    }
    if methods > 1 {
        return Err(Error::Usage {
            message: "choose only one of --host, --name, --group, --all".into(),
        });
    }

    if let Some(host) = &sel.host {
        return Ok(vec![attach(host.clone(), host.clone(), credentials)]);
    }

    if let Some(name) = &sel.name {
        let cache = DeviceCache::load(cache_path)?;
        let dev = cache.find(name).ok_or_else(|| Error::NotFound {
            message: format!("no cached device named `{name}` (run `tasmota discover`)"),
        })?;
        return Ok(vec![attach(
            dev.host.clone(),
            dev.name.clone(),
            credentials,
        )]);
    }

    if sel.all {
        let cache = DeviceCache::load(cache_path)?;
        if cache.devices.is_empty() {
            return Err(Error::NotFound {
                message: "no cached devices; run `tasmota discover` first".into(),
            });
        }
        return Ok(cache
            .devices
            .iter()
            .map(|d| attach(d.host.clone(), d.name.clone(), credentials))
            .collect());
    }

    // group
    let group = sel.group.as_ref().unwrap();
    let groups = load_groups(groups_path)?;
    let members = groups.groups.get(group).ok_or_else(|| Error::NotFound {
        message: format!("no group named `{group}` in {}", groups_path.display()),
    })?;
    if members.is_empty() {
        return Err(Error::Usage {
            message: format!("group `{group}` has no members"),
        });
    }
    let cache = DeviceCache::load(cache_path)?;
    members
        .iter()
        .map(|m| resolve_member(m, &cache, credentials))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_selection_is_usage_error() {
        let sel = Selector::default();
        let err = resolve(
            &sel,
            Path::new("/nonexistent/cache.json"),
            Path::new("/nonexistent/groups.toml"),
            &None,
        )
        .unwrap_err();
        assert_eq!(err.kind(), "usage");
    }

    #[test]
    fn two_selections_is_usage_error() {
        let sel = Selector {
            host: Some("192.0.2.1".into()),
            all: true,
            ..Default::default()
        };
        let err = resolve(
            &sel,
            Path::new("/nonexistent/cache.json"),
            Path::new("/nonexistent/groups.toml"),
            &None,
        )
        .unwrap_err();
        assert_eq!(err.kind(), "usage");
    }

    #[test]
    fn groups_parse_error_includes_an_example() {
        let dir = std::env::temp_dir().join("tasmota-cli-groups-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad-groups.toml");
        // Wrong shape: array-of-tables instead of a [groups] map.
        std::fs::write(&path, "[[groups]]\nname = \"x\"\n").unwrap();
        let err = list_groups(&path).unwrap_err();
        assert_eq!(err.kind(), "io");
        assert!(
            err.to_string().contains("[groups]") && err.to_string().contains("kitchen ="),
            "error should show the expected format: {err}"
        );
    }

    #[test]
    fn host_resolves_directly() {
        let sel = Selector {
            host: Some("198.51.100.5".into()),
            ..Default::default()
        };
        let out = resolve(
            &sel,
            Path::new("/nonexistent/cache.json"),
            Path::new("/nonexistent/groups.toml"),
            &None,
        )
        .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].addr.host, "198.51.100.5");
    }
}
