//! Higher-level device operations built on a [`Transport`].
//!
//! Each returns typed results with absent data preserved as `None`. Destructive
//! operations (firmware, restore, template apply) are exposed here as plain calls;
//! the confirmation guardrails live in the CLI layer around them.

use serde_json::Value;

use crate::error::{Error, Result};
use crate::model::{DeviceStatus, Relay};
use crate::parse::{StateTextMap, normalize_relay, parse_status, relay_index};
use crate::transport::{DeviceAddr, Transport};

/// A power action for a relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    On,
    Off,
    Toggle,
}

impl PowerAction {
    fn as_cmd(self) -> &'static str {
        match self {
            PowerAction::On => "ON",
            PowerAction::Off => "OFF",
            PowerAction::Toggle => "TOGGLE",
        }
    }
}

/// Parse an already-fetched `Status 0` value, resolving custom `StateText` labels
/// only when a relay string did not map with the defaults.
pub fn status_from_value<T: Transport>(
    t: &T,
    addr: &DeviceAddr,
    value: &Value,
) -> Result<DeviceStatus> {
    let prelim = parse_status(&addr.host, value, None)?;
    // No relays: nothing to normalize, skip the extra request.
    if prelim.relays.is_empty() {
        return Ok(prelim);
    }
    // Always resolve the device's configured relay labels. Custom `StateText` can
    // even alias the default `ON`/`OFF` words with inverted meaning, so a
    // default-looking string cannot be trusted without the map.
    match t.command(addr, "StateText") {
        Ok(stv) => {
            let map = StateTextMap::from_value(&stv);
            parse_status(&addr.host, value, Some(&map))
        }
        // A genuine no-answer (unreachable/timeout) mid-sequence is expected: fall
        // back to Unknown relays (raw preserved) rather than trust the defaults. A
        // command-level rejection / auth / parse error is a real failure: propagate.
        Err(Error::Network { .. }) => Ok(mark_relays_unknown(prelim)),
        Err(e) => Err(e),
    }
}

/// Replace every relay's state with `Unknown(raw)`: used when the relay label
/// mapping cannot be confirmed, so a possibly-wrong on/off is never reported.
fn mark_relays_unknown(mut status: DeviceStatus) -> DeviceStatus {
    for relay in &mut status.relays {
        relay.state = crate::model::RelayState::Unknown(relay.raw.clone());
    }
    status
}

/// Fetch and parse the full device status (`Status 0`).
pub fn get_status<T: Transport>(t: &T, addr: &DeviceAddr) -> Result<DeviceStatus> {
    let value = t.command(addr, "Status 0")?;
    status_from_value(t, addr, &value)
}

/// Extract the relay state from a `Power` command response (`{"POWER":"ON"}`),
/// normalizing with the device's `StateText` map when provided.
fn relay_from_response(value: &Value, map: Option<&StateTextMap>) -> Option<Relay> {
    let obj = value.as_object()?;
    for (k, v) in obj {
        if let Some(idx) = relay_index(k)
            && let Some(raw) = v.as_str()
        {
            return Some(Relay {
                index: idx,
                state: normalize_relay(raw, map),
                raw: raw.to_string(),
            });
        }
    }
    None
}

/// Set a relay's power. `relay` is `None`/`Some(0)` for the primary relay.
pub fn set_power<T: Transport>(
    t: &T,
    addr: &DeviceAddr,
    relay: Option<u8>,
    action: PowerAction,
) -> Result<Relay> {
    let idx = match relay {
        None | Some(0) => String::new(),
        Some(n) => n.to_string(),
    };
    let cmnd = format!("Power{idx} {}", action.as_cmd());
    let value = t.command(addr, &cmnd)?;
    // Resolve the device's configured relay labels (mirrors `status_from_value`) so a
    // custom or even inverted `StateText` normalizes the response correctly.
    let relay = match t.command(addr, "StateText") {
        Ok(stv) => {
            let map = StateTextMap::from_value(&stv);
            relay_from_response(&value, Some(&map))
        }
        // No-answer: keep the raw text as Unknown. A rejection/auth/parse error is a
        // real failure and propagates.
        Err(Error::Network { .. }) => relay_from_response(&value, None).map(|mut r| {
            r.state = crate::model::RelayState::Unknown(r.raw.clone());
            r
        }),
        Err(e) => return Err(e),
    };
    relay.ok_or_else(|| Error::Parse {
        message: format!("{} did not report a relay state after `{cmnd}`", addr.host),
    })
}

/// The firmware version string. A `Status 2` response without `StatusFWR.Version`
/// is not a valid Tasmota reply, so this errors rather than returning a missing
/// value that would render as a plausible `n/a` with exit 0.
pub fn firmware_version<T: Transport>(t: &T, addr: &DeviceAddr) -> Result<String> {
    let value = t.command(addr, "Status 2")?;
    value
        .get("StatusFWR")
        .and_then(|f| f.get("Version"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| Error::Parse {
            message: format!(
                "{} did not return a firmware version (not a Tasmota Status 2 response)",
                addr.host
            ),
        })
}

/// Fetch MQTT config/health from `Status 6` (`StatusMQT`). More reliable than
/// relying on `Status 0` to include the MQTT block. The connected flag stays `None`.
pub fn mqtt_info<T: Transport>(t: &T, addr: &DeviceAddr) -> Result<Option<crate::model::MqttInfo>> {
    let value = t.command(addr, "Status 6")?;
    Ok(crate::parse::mqtt_from_status(&value))
}

/// Point the device at an OTA URL (if given) and trigger an upgrade. Destructive:
/// the CLI must have confirmed before calling this.
///
/// Tasmota reports OTA failures as HTTP 200 with an `Upgrade` value like
/// `"Failed Verify Bin Header Failed"`, so a failed value is mapped to a rejection.
pub fn firmware_update<T: Transport>(
    t: &T,
    addr: &DeviceAddr,
    ota_url: Option<&str>,
) -> Result<Value> {
    if let Some(url) = ota_url {
        t.command(addr, &format!("OtaUrl {url}"))?;
    }
    let response = t.command(addr, "Upgrade 1")?;
    if let Some(status) = response.get("Upgrade").and_then(Value::as_str) {
        let lower = status.to_ascii_lowercase();
        if lower.contains("fail") || lower.contains("error") {
            return Err(Error::CommandRejected {
                command: "Upgrade 1".into(),
                message: format!("OTA rejected: {status}"),
            });
        }
    }
    Ok(response)
}

/// Download the device's binary configuration backup (`.dmp`) from `/dl`.
pub fn backup_config<T: Transport>(t: &T, addr: &DeviceAddr) -> Result<Vec<u8>> {
    t.download(addr, "/dl")
}

/// Read a single setting by issuing its command with no argument.
pub fn config_get<T: Transport>(t: &T, addr: &DeviceAddr, setting: &str) -> Result<Value> {
    t.command(addr, setting)
}

/// Write a setting (`<Setting> <value>`). Destructive-ish: guarded by the CLI.
pub fn config_set<T: Transport>(
    t: &T,
    addr: &DeviceAddr,
    setting: &str,
    value: &str,
) -> Result<Value> {
    t.command(addr, &format!("{setting} {value}"))
}

/// Send an arbitrary console command and return the raw JSON response.
pub fn console<T: Transport>(t: &T, addr: &DeviceAddr, command: &str) -> Result<Value> {
    t.command(addr, command)
}

/// Read the current GPIO template.
pub fn template_get<T: Transport>(t: &T, addr: &DeviceAddr) -> Result<Value> {
    t.command(addr, "Template")
}

/// Apply a GPIO template and activate it. Destructive: guarded by the CLI.
pub fn template_apply<T: Transport>(
    t: &T,
    addr: &DeviceAddr,
    template_json: &str,
) -> Result<Value> {
    t.command(addr, &format!("Template {template_json}"))?;
    t.command(addr, "Module 0")
}

/// Restore a binary config backup by uploading it to the device's restore
/// endpoint. Destructive, and the endpoint is unverified: the CLI must confirm and
/// warn before calling this.
pub fn restore_config<T: Transport>(t: &T, addr: &DeviceAddr, data: &[u8]) -> Result<Value> {
    t.upload(addr, "/u2", "u2", "config.dmp", data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result as CoreResult;
    use serde_json::json;
    use std::cell::RefCell;

    /// A scripted transport: returns queued responses and records commands.
    struct FakeTransport {
        responses: RefCell<Vec<Value>>,
        commands: RefCell<Vec<String>>,
    }

    impl FakeTransport {
        fn new(responses: Vec<Value>) -> Self {
            FakeTransport {
                responses: RefCell::new(responses),
                commands: RefCell::new(Vec::new()),
            }
        }
    }

    impl Transport for FakeTransport {
        fn command(&self, _addr: &DeviceAddr, cmnd: &str) -> CoreResult<Value> {
            self.commands.borrow_mut().push(cmnd.to_string());
            let mut r = self.responses.borrow_mut();
            if r.is_empty() {
                // Simulates a device that does not answer a follow-up (e.g. StateText).
                return Err(crate::error::Error::Network {
                    message: "no scripted response".into(),
                });
            }
            Ok(r.remove(0))
        }
        fn download(&self, _addr: &DeviceAddr, _path: &str) -> CoreResult<Vec<u8>> {
            Ok(b"dmp".to_vec())
        }
        fn upload(
            &self,
            _addr: &DeviceAddr,
            _path: &str,
            _field: &str,
            _filename: &str,
            _data: &[u8],
        ) -> CoreResult<Value> {
            Ok(json!({"restored": true}))
        }
    }

    #[test]
    fn set_power_toggle_reports_new_state() {
        let t = FakeTransport::new(vec![
            json!({"POWER": "ON"}),
            json!({"StateText1": "OFF", "StateText2": "ON"}),
        ]);
        let addr = DeviceAddr::new("192.0.2.30");
        let relay = set_power(&t, &addr, None, PowerAction::Toggle).unwrap();
        assert_eq!(relay.index, 0);
        assert_eq!(relay.state, crate::model::RelayState::On);
        assert_eq!(t.commands.borrow()[0], "Power TOGGLE");
    }

    #[test]
    fn set_power_response_respects_inverted_statetext() {
        // Response "ON" but StateText1(off)="ON": the relay is actually off.
        let t = FakeTransport::new(vec![
            json!({"POWER": "ON"}),
            json!({"StateText1": "ON", "StateText2": "OFF"}),
        ]);
        let addr = DeviceAddr::new("192.0.2.37");
        let relay = set_power(&t, &addr, None, PowerAction::Off).unwrap();
        assert_eq!(relay.state, crate::model::RelayState::Off);
    }

    #[test]
    fn set_power_resolves_custom_statetext_in_response() {
        // Response uses a custom label; StateText is fetched to normalize it.
        let t = FakeTransport::new(vec![
            json!({"POWER": "Open"}),
            json!({"StateText1": "Closed", "StateText2": "Open"}),
        ]);
        let addr = DeviceAddr::new("192.0.2.36");
        let relay = set_power(&t, &addr, None, PowerAction::On).unwrap();
        assert_eq!(relay.state, crate::model::RelayState::On);
        let cmds = t.commands.borrow();
        assert_eq!(cmds[0], "Power ON");
        assert_eq!(cmds[1], "StateText");
    }

    #[test]
    fn set_power_indexed_relay_builds_powern() {
        let t = FakeTransport::new(vec![
            json!({"POWER2": "OFF"}),
            json!({"StateText1": "OFF", "StateText2": "ON"}),
        ]);
        let addr = DeviceAddr::new("192.0.2.31");
        let relay = set_power(&t, &addr, Some(2), PowerAction::Off).unwrap();
        assert_eq!(relay.index, 2);
        assert_eq!(relay.state, crate::model::RelayState::Off);
        assert_eq!(t.commands.borrow()[0], "Power2 OFF");
    }

    #[test]
    fn firmware_update_sets_url_then_upgrades() {
        let t = FakeTransport::new(vec![
            json!({"OtaUrl": "http://x"}),
            json!({"Upgrade": "..."}),
        ]);
        let addr = DeviceAddr::new("192.0.2.32");
        firmware_update(&t, &addr, Some("http://x/f.bin")).unwrap();
        let cmds = t.commands.borrow();
        assert_eq!(cmds[0], "OtaUrl http://x/f.bin");
        assert_eq!(cmds[1], "Upgrade 1");
    }

    #[test]
    fn firmware_update_rejects_failed_ota() {
        // Tasmota returns HTTP 200 with a Failed Upgrade value; must not be success.
        let t = FakeTransport::new(vec![
            json!({"OtaUrl": "http://x"}),
            json!({"Upgrade": "Failed Verify Bin Header Failed"}),
        ]);
        let addr = DeviceAddr::new("192.0.2.43");
        let err = firmware_update(&t, &addr, Some("http://x/f.bin")).unwrap_err();
        assert_eq!(err.kind(), "command_rejected");
    }

    #[test]
    fn firmware_version_requires_statusfwr() {
        let ok = FakeTransport::new(vec![json!({"StatusFWR": {"Version": "14.2.0"}})]);
        assert_eq!(
            firmware_version(&ok, &DeviceAddr::new("192.0.2.41")).unwrap(),
            "14.2.0"
        );
        // A non-Tasmota 200 body must error, not return a plausible n/a.
        let bad = FakeTransport::new(vec![json!({"foo": "bar"})]);
        assert!(firmware_version(&bad, &DeviceAddr::new("192.0.2.42")).is_err());
    }

    #[test]
    fn mqtt_info_parses_status6() {
        let t = FakeTransport::new(vec![json!({
            "StatusMQT": {"MqttHost": "192.0.2.1", "MqttPort": 1883, "MqttCount": 2}
        })]);
        let addr = DeviceAddr::new("192.0.2.40");
        let m = mqtt_info(&t, &addr).unwrap().unwrap();
        assert_eq!(m.host.as_deref(), Some("192.0.2.1"));
        assert_eq!(m.port, Some(1883));
        assert_eq!(m.connected, None);
        assert_eq!(t.commands.borrow()[0], "Status 6");
    }

    #[test]
    fn get_status_applies_statetext_for_relay_device() {
        let t = FakeTransport::new(vec![
            json!({"StatusFWR": {"Version": "14.2.0"}, "StatusSTS": {"POWER": "ON"}}),
            json!({"StateText1": "OFF", "StateText2": "ON"}),
        ]);
        let addr = DeviceAddr::new("192.0.2.33");
        let status = get_status(&t, &addr).unwrap();
        assert_eq!(status.relays[0].state, crate::model::RelayState::On);
        let cmds = t.commands.borrow();
        assert_eq!(cmds[0], "Status 0");
        assert_eq!(cmds[1], "StateText");
    }

    #[test]
    fn status_marks_relays_unknown_when_statetext_unavailable() {
        // Only Status 0 is answered; the StateText follow-up errors, so the relay
        // must be Unknown (raw preserved), never a guessed on/off.
        let t = FakeTransport::new(vec![json!({
            "StatusFWR": {"Version": "14.2.0"},
            "StatusSTS": {"POWER": "ON"}
        })]);
        let addr = DeviceAddr::new("192.0.2.50");
        let s = get_status(&t, &addr).unwrap();
        assert!(matches!(
            s.relays[0].state,
            crate::model::RelayState::Unknown(_)
        ));
        assert_eq!(s.relays[0].raw, "ON");
    }

    #[test]
    fn set_power_marks_unknown_when_statetext_unavailable() {
        let t = FakeTransport::new(vec![json!({"POWER": "ON"})]);
        let addr = DeviceAddr::new("192.0.2.51");
        let relay = set_power(&t, &addr, None, PowerAction::On).unwrap();
        assert!(matches!(relay.state, crate::model::RelayState::Unknown(_)));
    }

    #[test]
    fn status_propagates_statetext_command_rejection() {
        // A StateText that is command-rejected (not a network no-answer) must fail,
        // not be swallowed into a success with Unknown relays.
        struct RejectStateText;
        impl Transport for RejectStateText {
            fn command(&self, _addr: &DeviceAddr, cmnd: &str) -> CoreResult<Value> {
                if cmnd == "StateText" {
                    Err(crate::error::Error::CommandRejected {
                        command: "StateText".into(),
                        message: "rejected".into(),
                    })
                } else {
                    Ok(json!({"StatusFWR": {"Version": "14.2.0"}, "StatusSTS": {"POWER": "ON"}}))
                }
            }
            fn download(&self, _addr: &DeviceAddr, _path: &str) -> CoreResult<Vec<u8>> {
                Ok(Vec::new())
            }
            fn upload(
                &self,
                _addr: &DeviceAddr,
                _path: &str,
                _field: &str,
                _filename: &str,
                _data: &[u8],
            ) -> CoreResult<Value> {
                Ok(Value::Null)
            }
        }
        let err = get_status(&RejectStateText, &DeviceAddr::new("192.0.2.52")).unwrap_err();
        assert_eq!(err.kind(), "command_rejected");
    }

    #[test]
    fn inverted_statetext_flips_default_looking_labels() {
        // A device that inverts the defaults: StateText1 (off) = "ON".
        let t = FakeTransport::new(vec![
            json!({"StatusFWR": {"Version": "14.2.0"}, "StatusSTS": {"POWER": "ON"}}),
            json!({"StateText1": "ON", "StateText2": "OFF"}),
        ]);
        let addr = DeviceAddr::new("192.0.2.34");
        let status = get_status(&t, &addr).unwrap();
        // POWER="ON" but the device says StateText1(off)="ON": it is actually off.
        assert_eq!(status.relays[0].state, crate::model::RelayState::Off);
    }

    #[test]
    fn sensor_only_device_skips_statetext() {
        // No relays in StatusSTS: only one request, no StateText fetch.
        let t = FakeTransport::new(vec![json!({
            "StatusFWR": {"Version": "14.2.0"},
            "StatusSTS": {"Uptime": "1T00:00:00"}
        })]);
        let addr = DeviceAddr::new("192.0.2.35");
        let status = get_status(&t, &addr).unwrap();
        assert!(status.relays.is_empty());
        assert_eq!(t.commands.borrow().len(), 1);
    }
}
