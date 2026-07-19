//! `impl switchkit::SmartDevice for tasmota_core::HttpTransport`.
//!
//! Maps the Tasmota-specific `DeviceStatus`/`Error` model onto switchkit's
//! vendor-neutral `DeviceSnapshot`/`Error`. Absent data stays absent: a missing
//! `Energy` block maps to `None` (never a zeroed `Energy`), an unmapped relay
//! label maps to `RelayState::Unknown` (never a guessed `Off`), and offline/
//! unreachable is always `Err`, never a field on a "successful" snapshot.

use serde_json::Value;

use switchkit::{
    Capabilities, DeviceSnapshot, DeviceTarget, Energy as SkEnergy, Firmware, NetInfo as SkNetInfo,
    PowerAction as SkPowerAction, Relay as SkRelay, RelayState as SkRelayState, Signal,
    SmartDevice, Vendor,
};

use crate::model::{DeviceStatus, RelayState as TmRelayState};
use crate::ops::{self, PowerAction as TmPowerAction};
use crate::transport::{Credentials, DeviceAddr, HttpTransport, Transport};

/// Map a vendor-neutral target to the Tasmota-specific address, translating
/// credentials field-for-field (never reconstructing a URL from them).
fn to_addr(target: &DeviceTarget) -> DeviceAddr {
    DeviceAddr::new(target.host.clone()).with_credentials(target.credentials.as_ref().map(|c| {
        Credentials {
            user: c.user.clone(),
            password: c.password.clone(),
        }
    }))
}

/// The device's primary display name: `DeviceName`, else the first configured
/// friendly name. Unlike `DeviceStatus::display_name`, this never falls back to
/// the host - a fabricated "name" that is really just the address is not a name.
fn primary_name(status: &DeviceStatus) -> Option<String> {
    status
        .name
        .clone()
        .filter(|n| !n.is_empty())
        .or_else(|| status.friendly_names.first().cloned())
}

/// Map a parsed `DeviceStatus` onto a `switchkit::DeviceSnapshot`. Every leaf that
/// was absent in the source stays absent here; nothing is defaulted to a
/// plausible-looking value.
fn snapshot_from_status(status: DeviceStatus) -> DeviceSnapshot {
    // Computed before any field of `status` is moved out below: both borrow the
    // whole struct, which a partial move (e.g. `host: status.host`) would forbid.
    let name = primary_name(&status);
    let model = status.module.map(|m| m.to_string());

    let relays: Vec<SkRelay> = status
        .relays
        .into_iter()
        .map(|r| SkRelay {
            index: r.index,
            state: match r.state {
                TmRelayState::On => SkRelayState::On,
                TmRelayState::Off => SkRelayState::Off,
                TmRelayState::Unknown(raw) => SkRelayState::Unknown(raw),
            },
            raw: r.raw,
        })
        .collect();

    // `None` only when the device has no energy sensor at all; a present block
    // with individually-missing readings keeps each leaf `Option` as parsed.
    let energy = status.energy.map(|e| SkEnergy {
        power_w: e.power_w,
        today_kwh: e.today_kwh,
        total_kwh: e.total_kwh,
        voltage_v: e.voltage_v,
        current_a: e.current_a,
    });

    // Tasmota's `Wifi.RSSI` is already a 0-100 quality percentage, never a dBm
    // value: map with `from_quality_percent`, never fabricate a dBm reading.
    let signal = status.wifi_rssi.map(Signal::from_quality_percent);

    // `None` when the device did not report a firmware version at all (should not
    // happen for a confirmed Tasmota response, but this must not be coerced into a
    // "known" firmware block with a `None` version).
    let firmware = status.firmware.map(|version| Firmware {
        version: Some(version),
        // Tasmota's `Status 0` does not report OTA-update availability.
        update_available: None,
    });

    let net = SkNetInfo {
        ip: status.net.ip,
        mac: status.net.mac,
        hostname: status.net.hostname,
    };

    let capabilities = Capabilities {
        metering: energy.is_some(),
        multi_channel: relays.len() > 1,
        // Every Tasmota device reachable over `/cm` supports OTA, config backup,
        // and raw console commands: these are firmware-wide, not per-device.
        firmware_ota: true,
        config_backup: true,
        console: true,
    };

    DeviceSnapshot {
        host: status.host,
        name,
        // Tasmota's `Module` is a numeric GPIO-template id, the closest analog to
        // a model identifier the core exposes (no name lookup table exists).
        model,
        generation: None,
        capabilities,
        relays,
        energy,
        signal,
        temperature_c: None,
        firmware,
        net,
        uptime: status.uptime,
    }
}

/// Map a `tasmota_core::Error` (already credential-scrubbed by the transport
/// layer) onto the vendor-neutral `switchkit::Error`, attaching `host`.
///
/// Exhaustive over every variant in `crate::error::Error`, with NO trailing `_`
/// arm: `tasmota_core::Error` is a plain (non-`#[non_exhaustive]`) enum owned by
/// this workspace, so a future variant fails this match at compile time rather
/// than silently falling through a catch-all - the exhaustiveness itself is the
/// safety net.
fn map_err(e: crate::error::Error, host: &str) -> switchkit::Error {
    use crate::error::Error as TmError;

    let message = e.to_string();
    let host = host.to_string();
    match e {
        TmError::Network { .. } | TmError::Io { .. } => switchkit::Error::Network { host, message },
        TmError::Auth { .. } => switchkit::Error::Auth { host, message },
        TmError::CommandRejected { .. } | TmError::NotFound { .. } | TmError::Aborted { .. } => {
            switchkit::Error::Rejected { host, message }
        }
        TmError::Parse { .. } => switchkit::Error::Parse { host, message },
        TmError::Usage { .. } | TmError::Unavailable { .. } => {
            switchkit::Error::Unsupported { host, message }
        }
    }
}

impl SmartDevice for HttpTransport {
    fn vendor(&self) -> Vendor {
        Vendor::Tasmota
    }

    fn probe(&self, target: &DeviceTarget) -> switchkit::Result<Option<DeviceSnapshot>> {
        let addr = to_addr(target);
        match self.command(&addr, "Status 0") {
            Ok(v) => {
                if crate::parse::looks_like_tasmota(&v) {
                    let status = ops::status_from_value(self, &addr, &v)
                        .map_err(|e| map_err(e, &addr.host))?;
                    Ok(Some(snapshot_from_status(status)))
                } else {
                    // Reachable, answered JSON, but not the Tasmota signature: a
                    // reachable non-match, never a guessed vendor.
                    Ok(None)
                }
            }
            // The host answered but not with a Tasmota `/cm` response (non-JSON
            // body, or a rejected command): reachable non-match, not an error.
            Err(crate::error::Error::Parse { .. })
            | Err(crate::error::Error::CommandRejected { .. }) => Ok(None),
            Err(e) => Err(map_err(e, &addr.host)),
        }
    }

    fn status(&self, target: &DeviceTarget) -> switchkit::Result<DeviceSnapshot> {
        let addr = to_addr(target);
        let status = ops::get_status(self, &addr).map_err(|e| map_err(e, &addr.host))?;
        Ok(snapshot_from_status(status))
    }

    fn set_power(
        &self,
        target: &DeviceTarget,
        channel: Option<u8>,
        action: SkPowerAction,
    ) -> switchkit::Result<SkRelay> {
        let addr = to_addr(target);
        let action = match action {
            SkPowerAction::On => TmPowerAction::On,
            SkPowerAction::Off => TmPowerAction::Off,
            SkPowerAction::Toggle => TmPowerAction::Toggle,
        };
        let relay =
            ops::set_power(self, &addr, channel, action).map_err(|e| map_err(e, &addr.host))?;
        Ok(SkRelay {
            index: relay.index,
            state: match relay.state {
                TmRelayState::On => SkRelayState::On,
                TmRelayState::Off => SkRelayState::Off,
                TmRelayState::Unknown(raw) => SkRelayState::Unknown(raw),
            },
            raw: relay.raw,
        })
    }

    fn firmware_version(&self, target: &DeviceTarget) -> switchkit::Result<Option<String>> {
        let addr = to_addr(target);
        let version = ops::firmware_version(self, &addr).map_err(|e| map_err(e, &addr.host))?;
        Ok(Some(version))
    }

    fn firmware_update(
        &self,
        target: &DeviceTarget,
        ota_url: Option<&str>,
    ) -> switchkit::Result<()> {
        let addr = to_addr(target);
        // `ops::firmware_update` returns the raw `Value` response; its OTA-failure
        // `Err` (an `Error::CommandRejected`) propagates via `?` below, the `Value`
        // itself is discarded since the trait signature reports only success/error.
        ops::firmware_update(self, &addr, ota_url).map_err(|e| map_err(e, &addr.host))?;
        Ok(())
    }

    fn config_get(&self, target: &DeviceTarget, setting: &str) -> switchkit::Result<Value> {
        let addr = to_addr(target);
        ops::config_get(self, &addr, setting).map_err(|e| map_err(e, &addr.host))
    }

    fn config_set(
        &self,
        target: &DeviceTarget,
        setting: &str,
        value: &str,
    ) -> switchkit::Result<Value> {
        let addr = to_addr(target);
        ops::config_set(self, &addr, setting, value).map_err(|e| map_err(e, &addr.host))
    }

    fn backup(&self, target: &DeviceTarget) -> switchkit::Result<Vec<u8>> {
        let addr = to_addr(target);
        ops::backup_config(self, &addr).map_err(|e| map_err(e, &addr.host))
    }

    fn console(&self, target: &DeviceTarget, command: &str) -> switchkit::Result<Value> {
        let addr = to_addr(target);
        ops::console(self, &addr, command).map_err(|e| map_err(e, &addr.host))
    }
}
