//! Device model produced from a Tasmota `Status 0` response.
//!
//! Absent data is represented with `Option::None` (rendered `null`/`n/a`), never
//! coerced to `0` or an empty string. A missing energy block means "no energy
//! sensor", not "0 watts"; an unrecognized relay string is `Unknown`, not `Off`.

use serde::{Serialize, Serializer};

/// Live state of a single relay, taken from `StatusSTS.POWER`/`POWERn`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayState {
    On,
    Off,
    /// The device reported a relay string we could not confidently map to on/off
    /// (e.g. a custom `StateText`). The raw text is preserved; never treated as off.
    Unknown(String),
}

impl RelayState {
    pub fn as_str(&self) -> &str {
        match self {
            RelayState::On => "on",
            RelayState::Off => "off",
            RelayState::Unknown(_) => "unknown",
        }
    }
}

impl Serialize for RelayState {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// A relay and its live state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Relay {
    /// `0` for a single-relay `POWER`, otherwise the 1-based index of `POWERn`.
    pub index: u8,
    pub state: RelayState,
    /// The raw relay string as reported, kept for transparency.
    pub raw: String,
}

/// Energy readings from `StatusSNS.ENERGY`. The whole struct is absent (`None`
/// at the call site) when the device has no energy sensor. Individual fields are
/// `None` when that specific field is missing.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct Energy {
    pub power_w: Option<f64>,
    pub voltage_v: Option<f64>,
    pub current_a: Option<f64>,
    pub today_kwh: Option<f64>,
    pub yesterday_kwh: Option<f64>,
    pub total_kwh: Option<f64>,
}

/// Network identity from `StatusNET`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct NetInfo {
    pub ip: Option<String>,
    pub mac: Option<String>,
    pub hostname: Option<String>,
}

/// MQTT config/health from `StatusMQT` (`Status 6`). Tasmota exposes no reliable
/// live connected/disconnected flag over HTTP, so `connected` stays `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct MqttInfo {
    pub host: Option<String>,
    pub port: Option<i64>,
    pub client: Option<String>,
    pub reconnect_count: Option<i64>,
    pub connected: Option<bool>,
}

/// The full parsed picture of a device from `Status 0`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DeviceStatus {
    /// The host (ip or hostname) the status was read from.
    pub host: String,
    pub name: Option<String>,
    pub friendly_names: Vec<String>,
    pub module: Option<i64>,
    /// Live relay states from `StatusSTS`. Empty when the device exposes none.
    pub relays: Vec<Relay>,
    pub firmware: Option<String>,
    pub net: NetInfo,
    pub uptime: Option<String>,
    pub wifi_rssi: Option<i64>,
    /// `None` means the device has no energy sensor (not zero power).
    pub energy: Option<Energy>,
    pub mqtt: Option<MqttInfo>,
}

impl DeviceStatus {
    /// The device's primary display name: `DeviceName`, else the first friendly
    /// name, else the host.
    pub fn display_name(&self) -> &str {
        self.name
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| self.friendly_names.first().map(|s| s.as_str()))
            .unwrap_or(&self.host)
    }
}
