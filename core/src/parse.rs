//! Parse a Tasmota `Status 0` response into [`DeviceStatus`].
//!
//! Hard rules (each has a regression test below):
//! - Live relay state comes from `StatusSTS.POWER` / `POWERn`, never from the
//!   numeric `Status.Power` bitmask.
//! - Relay text defaults to `ON`/`OFF` but is configurable via `StateText`; an
//!   unrecognized string is `Unknown`, never coerced to `Off`.
//! - A missing `StatusSNS.ENERGY` block yields `None` (no sensor), not zero.
//! - `Wifi.RSSI` is nested under `StatusSTS.Wifi`.
//! - MQTT comes from `StatusMQT`; `connected` is never inferred over HTTP.

use serde_json::Value;

use crate::error::{Error, Result};
use crate::model::{DeviceStatus, Energy, MqttInfo, NetInfo, Relay, RelayState};

/// The device's configured relay state labels, from the `StateText` command
/// (`{"StateText1":"OFF","StateText2":"ON",...}`). Used to normalize custom labels.
#[derive(Debug, Clone, Default)]
pub struct StateTextMap {
    pub off: Option<String>,
    pub on: Option<String>,
}

impl StateTextMap {
    /// Parse the JSON returned by the `StateText` command.
    pub fn from_value(v: &Value) -> Self {
        StateTextMap {
            off: v
                .get("StateText1")
                .and_then(Value::as_str)
                .map(str::to_owned),
            on: v
                .get("StateText2")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }
    }

    fn is_empty(&self) -> bool {
        self.off.is_none() && self.on.is_none()
    }
}

/// Normalize a raw relay string to [`RelayState`].
///
/// When a non-empty `StateText` map is supplied, the string is mapped ONLY against
/// that map: a value that matches neither label (or an ambiguous map with identical
/// labels) is `Unknown`, never coerced via the built-in `ON`/`OFF` defaults. The
/// built-in defaults apply only when no map was obtained.
pub fn normalize_relay(raw: &str, map: Option<&StateTextMap>) -> RelayState {
    let t = raw.trim();

    if let Some(m) = map.filter(|m| !m.is_empty()) {
        // Ambiguous map (identical on/off labels): cannot confidently decide.
        if let (Some(on), Some(off)) = (&m.on, &m.off)
            && on.eq_ignore_ascii_case(off)
        {
            return RelayState::Unknown(t.to_string());
        }
        if let Some(on) = &m.on
            && t.eq_ignore_ascii_case(on)
        {
            return RelayState::On;
        }
        if let Some(off) = &m.off
            && t.eq_ignore_ascii_case(off)
        {
            return RelayState::Off;
        }
        // A map was supplied but the value matches neither label: not confident.
        return RelayState::Unknown(t.to_string());
    }

    match t.to_ascii_uppercase().as_str() {
        "ON" | "1" | "TRUE" => RelayState::On,
        "OFF" | "0" | "FALSE" => RelayState::Off,
        _ => RelayState::Unknown(t.to_string()),
    }
}

fn as_f64(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64)
}

fn as_i64(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(Value::as_i64)
}

fn as_string(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|s| !s.is_empty())
}

/// Extract friendly names, which Tasmota reports as either an array or a scalar.
fn friendly_names(status: &Value) -> Vec<String> {
    match status.get("FriendlyName") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

/// Collect relays from `StatusSTS`, reading only `POWER` / `POWERn` string fields.
fn parse_relays(sts: &Value, map: Option<&StateTextMap>) -> Vec<Relay> {
    let Some(obj) = sts.as_object() else {
        return Vec::new();
    };
    let mut relays: Vec<Relay> = Vec::new();
    for (key, val) in obj {
        let Some(index) = relay_index(key) else {
            continue;
        };
        // Only string relay values are live state. Ignore anything else so the
        // numeric `Status.Power` bitmask (a different object) can never leak in.
        let Some(raw) = val.as_str() else { continue };
        relays.push(Relay {
            index,
            state: normalize_relay(raw, map),
            raw: raw.to_string(),
        });
    }
    relays.sort_by_key(|r| r.index);
    relays
}

/// `POWER` -> 0, `POWER1`..`POWERn` -> n. Anything else -> None.
pub(crate) fn relay_index(key: &str) -> Option<u8> {
    let rest = key.strip_prefix("POWER")?;
    if rest.is_empty() {
        return Some(0);
    }
    rest.parse::<u8>().ok()
}

fn parse_energy(sns: &Value) -> Option<Energy> {
    let energy = sns.get("ENERGY")?;
    // Present block, but a field may still be individually absent.
    Some(Energy {
        power_w: as_f64(energy, "Power"),
        voltage_v: as_f64(energy, "Voltage"),
        current_a: as_f64(energy, "Current"),
        today_kwh: as_f64(energy, "Today"),
        yesterday_kwh: as_f64(energy, "Yesterday"),
        total_kwh: as_f64(energy, "Total"),
    })
}

fn parse_net(net: &Value) -> NetInfo {
    NetInfo {
        ip: as_string(net, "IPAddress"),
        mac: as_string(net, "Mac"),
        hostname: as_string(net, "Hostname"),
    }
}

fn parse_mqtt(mqt: &Value) -> MqttInfo {
    MqttInfo {
        host: as_string(mqt, "MqttHost"),
        port: as_i64(mqt, "MqttPort"),
        client: as_string(mqt, "MqttClient"),
        reconnect_count: as_i64(mqt, "MqttCount"),
        // Tasmota exposes no reliable live connected flag over HTTP.
        connected: None,
    }
}

/// Extract MQTT info from any status response carrying `StatusMQT` (e.g. `Status 6`).
pub fn mqtt_from_status(value: &Value) -> Option<MqttInfo> {
    value.get("StatusMQT").map(parse_mqtt)
}

/// Parse a `Status 0` JSON document. `statetext` may carry the device's configured
/// relay labels (from the `StateText` command) so custom labels resolve correctly.
pub fn parse_status(
    host: &str,
    value: &Value,
    statetext: Option<&StateTextMap>,
) -> Result<DeviceStatus> {
    if !value.is_object() {
        return Err(Error::Parse {
            message: format!("expected a JSON object from {host}, got a non-object response"),
        });
    }
    // A bare `{}` or an unrelated JSON object from a non-Tasmota endpoint must not
    // become a plausible-looking status with null fields.
    if !looks_like_tasmota(value) {
        return Err(Error::Parse {
            message: format!(
                "{host} did not return a Tasmota Status 0 response (no StatusFWR.Version)"
            ),
        });
    }

    let status = value.get("Status");
    let sts = value.get("StatusSTS");

    let relays = sts.map(|s| parse_relays(s, statetext)).unwrap_or_default();

    Ok(DeviceStatus {
        host: host.to_string(),
        name: status.and_then(|s| as_string(s, "DeviceName")),
        friendly_names: status.map(friendly_names).unwrap_or_default(),
        module: status.and_then(|s| as_i64(s, "Module")),
        relays,
        firmware: value.get("StatusFWR").and_then(|f| as_string(f, "Version")),
        net: value.get("StatusNET").map(parse_net).unwrap_or_default(),
        uptime: sts.and_then(|s| as_string(s, "Uptime")),
        wifi_rssi: sts
            .and_then(|s| s.get("Wifi"))
            .and_then(|w| as_i64(w, "RSSI")),
        energy: value.get("StatusSNS").and_then(parse_energy),
        mqtt: value.get("StatusMQT").map(parse_mqtt),
    })
}

/// A device is "Tasmota" for discovery purposes when its `Status 0` response has
/// the `StatusFWR` firmware object. Header sniffing is not used.
pub fn looks_like_tasmota(value: &Value) -> bool {
    value
        .get("StatusFWR")
        .and_then(|f| f.get("Version"))
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn single_relay_on_from_status_sts() {
        // Status.Power bitmask is 0, but the live state in StatusSTS.POWER is ON.
        // The bitmask must be ignored (negative control).
        let v = json!({
            "Status": {"DeviceName": "Plug", "Power": 0, "Module": 1, "FriendlyName": ["Plug"]},
            "StatusFWR": {"Version": "14.2.0"},
            "StatusSTS": {"POWER": "ON", "Uptime": "1T00:00:00", "Wifi": {"RSSI": 72}}
        });
        let d = parse_status("192.0.2.10", &v, None).unwrap();
        assert_eq!(d.relays.len(), 1);
        assert_eq!(d.relays[0].index, 0);
        assert_eq!(d.relays[0].state, RelayState::On);
        assert_eq!(d.wifi_rssi, Some(72));
        assert_eq!(d.firmware.as_deref(), Some("14.2.0"));
    }

    #[test]
    fn multi_relay_power1_power2() {
        let v = json!({
            "Status": {"DeviceName": "Strip", "Power": 2},
            "StatusFWR": {"Version": "14.2.0"},
            "StatusSTS": {"POWER1": "OFF", "POWER2": "ON", "POWER3": "OFF"}
        });
        let d = parse_status("192.0.2.11", &v, None).unwrap();
        assert_eq!(d.relays.len(), 3);
        assert_eq!(d.relays[0].index, 1);
        assert_eq!(d.relays[0].state, RelayState::Off);
        assert_eq!(d.relays[1].index, 2);
        assert_eq!(d.relays[1].state, RelayState::On);
    }

    #[test]
    fn custom_statetext_maps_to_on_off() {
        let map = StateTextMap {
            off: Some("Closed".into()),
            on: Some("Open".into()),
        };
        let v = json!({
            "StatusFWR": {"Version": "14.2.0"},
            "StatusSTS": {"POWER": "Open"}
        });
        let d = parse_status("192.0.2.12", &v, Some(&map)).unwrap();
        assert_eq!(d.relays[0].state, RelayState::On);
        assert_eq!(d.relays[0].raw, "Open");
    }

    #[test]
    fn statetext_map_present_but_no_match_is_unknown() {
        // With custom labels, a value that is not one of them (even "ON") is Unknown,
        // never coerced via the built-in defaults.
        let map = StateTextMap {
            off: Some("Closed".into()),
            on: Some("Open".into()),
        };
        assert_eq!(
            normalize_relay("ON", Some(&map)),
            RelayState::Unknown("ON".into())
        );
    }

    #[test]
    fn ambiguous_statetext_map_is_unknown() {
        let map = StateTextMap {
            off: Some("X".into()),
            on: Some("X".into()),
        };
        assert_eq!(
            normalize_relay("X", Some(&map)),
            RelayState::Unknown("X".into())
        );
    }

    #[test]
    fn unrecognized_relay_string_is_unknown_not_off() {
        // Negative control: an unmappable label must never become Off.
        let v = json!({
            "StatusFWR": {"Version": "14.2.0"},
            "StatusSTS": {"POWER": "Blinking"}
        });
        let d = parse_status("192.0.2.13", &v, None).unwrap();
        assert_eq!(d.relays[0].state, RelayState::Unknown("Blinking".into()));
        assert_ne!(d.relays[0].state, RelayState::Off);
    }

    #[test]
    fn missing_energy_block_is_none_not_zero() {
        let v = json!({
            "StatusFWR": {"Version": "14.2.0"},
            "StatusSTS": {"POWER": "ON"},
            "StatusSNS": {"Time": "2026-07-18T00:00:00"}
        });
        let d = parse_status("192.0.2.14", &v, None).unwrap();
        assert!(d.energy.is_none(), "no ENERGY block must be None, not 0");
    }

    #[test]
    fn energy_block_present_parses_fields() {
        let v = json!({
            "StatusFWR": {"Version": "14.2.0"},
            "StatusSTS": {"POWER": "ON"},
            "StatusSNS": {"ENERGY": {"Power": 42.5, "Voltage": 230, "Today": 1.234, "Total": 99.9}}
        });
        let d = parse_status("192.0.2.15", &v, None).unwrap();
        let e = d.energy.unwrap();
        assert_eq!(e.power_w, Some(42.5));
        assert_eq!(e.voltage_v, Some(230.0));
        assert_eq!(e.today_kwh, Some(1.234));
        assert_eq!(e.yesterday_kwh, None);
    }

    #[test]
    fn mqtt_connected_is_never_inferred() {
        let v = json!({
            "StatusFWR": {"Version": "14.2.0"},
            "StatusMQT": {"MqttHost": "192.0.2.1", "MqttPort": 1883, "MqttCount": 3}
        });
        let d = parse_status("192.0.2.16", &v, None).unwrap();
        let m = d.mqtt.unwrap();
        assert_eq!(m.host.as_deref(), Some("192.0.2.1"));
        assert_eq!(m.port, Some(1883));
        assert_eq!(m.connected, None);
    }

    #[test]
    fn tasmota_signature_requires_firmware_version() {
        assert!(looks_like_tasmota(
            &json!({"StatusFWR": {"Version": "14.2.0"}})
        ));
        assert!(!looks_like_tasmota(&json!({"something": "else"})));
        assert!(!looks_like_tasmota(&json!({"StatusFWR": {}})));
    }

    #[test]
    fn non_object_response_is_parse_error() {
        let v = json!("not an object");
        assert!(parse_status("192.0.2.17", &v, None).is_err());
    }

    #[test]
    fn non_tasmota_object_is_parse_error() {
        // An HTTP 200 with an unrelated or empty JSON object is not a success.
        assert!(parse_status("192.0.2.18", &json!({}), None).is_err());
        assert!(parse_status("192.0.2.19", &json!({"foo": "bar"}), None).is_err());
    }
}
