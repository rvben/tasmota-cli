//! Human-readable text rendering. JSON output serializes `tasmota-core` types
//! directly; these functions produce the `--output text` view.
//!
//! Absent values render as `n/a`, never `0`.

use tasmota_core::cache::CachedDevice;
use tasmota_core::discovery::Discovered;
use tasmota_core::model::{DeviceStatus, Energy};

/// Format an optional value, or `n/a` when absent.
pub fn na<T: std::fmt::Display>(v: Option<T>) -> String {
    match v {
        Some(v) => v.to_string(),
        None => "n/a".to_string(),
    }
}

fn relays_line(status: &DeviceStatus) -> String {
    if status.relays.is_empty() {
        return "relays:  none".to_string();
    }
    let parts: Vec<String> = status
        .relays
        .iter()
        .map(|r| {
            let label = if r.index == 0 {
                "power".to_string()
            } else {
                format!("power{}", r.index)
            };
            format!("{label}={}", r.state.as_str())
        })
        .collect();
    format!("relays:  {}", parts.join("  "))
}

/// Full device status block.
pub fn status(status: &DeviceStatus) -> String {
    let mut out = String::new();
    out.push_str(&format!("{} ({})\n", status.display_name(), status.host));
    out.push_str(&format!("  {}\n", relays_line(status)));
    out.push_str(&format!("  firmware: {}\n", na(status.firmware.as_deref())));
    out.push_str(&format!("  rssi:    {}\n", na(status.wifi_rssi)));
    out.push_str(&format!("  uptime:  {}\n", na(status.uptime.as_deref())));
    out.push_str(&format!("  ip:      {}\n", na(status.net.ip.as_deref())));
    out.push_str(&format!("  power:   {}", power_summary(status)));
    out
}

fn power_summary(status: &DeviceStatus) -> String {
    match &status.energy {
        None => "n/a (no energy sensor)".to_string(),
        Some(e) => format!("{} W", na(e.power_w)),
    }
}

/// One-line power reading.
pub fn power(status: &DeviceStatus) -> String {
    match &status.energy {
        None => format!("{}: n/a (no energy sensor)", status.display_name()),
        Some(e) => format!("{}: {} W", status.display_name(), na(e.power_w)),
    }
}

/// Energy totals.
pub fn energy(status: &DeviceStatus) -> String {
    match &status.energy {
        None => format!("{}: n/a (no energy sensor)", status.display_name()),
        Some(e) => energy_block(status.display_name(), e),
    }
}

fn energy_block(name: &str, e: &Energy) -> String {
    format!(
        "{name}\n  power:     {} W\n  voltage:   {} V\n  current:   {} A\n  today:     {} kWh\n  yesterday: {} kWh\n  total:     {} kWh",
        na(e.power_w),
        na(e.voltage_v),
        na(e.current_a),
        na(e.today_kwh),
        na(e.yesterday_kwh),
        na(e.total_kwh),
    )
}

/// Health summary.
pub fn health(status: &DeviceStatus) -> String {
    let mqtt = match &status.mqtt {
        None => "n/a".to_string(),
        Some(m) => format!(
            "host={} port={} (connected: n/a over HTTP)",
            na(m.host.as_deref()),
            na(m.port)
        ),
    };
    format!(
        "{} ({})\n  online:   yes\n  rssi:     {}\n  uptime:   {}\n  firmware: {}\n  mqtt:     {}",
        status.display_name(),
        status.host,
        na(status.wifi_rssi),
        na(status.uptime.as_deref()),
        na(status.firmware.as_deref()),
        mqtt,
    )
}

/// A cached device list.
pub fn devices(list: &[CachedDevice]) -> String {
    if list.is_empty() {
        return "no cached devices (run `tasmota discover`)".to_string();
    }
    let mut out = String::new();
    for d in list {
        out.push_str(&format!("{:<24} {}\n", d.name, d.host));
    }
    out.trim_end().to_string()
}

/// The result of a discovery scan.
pub fn discovered(list: &[Discovered]) -> String {
    if list.is_empty() {
        return "no Tasmota devices found".to_string();
    }
    let mut out = format!("found {} device(s):\n", list.len());
    for d in list {
        out.push_str(&format!(
            "  {:<24} {:<16} {}\n",
            d.status.display_name(),
            d.host,
            na(d.status.firmware.as_deref())
        ));
    }
    out.trim_end().to_string()
}
