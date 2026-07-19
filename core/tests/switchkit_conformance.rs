//! Conformance test for `impl switchkit::SmartDevice for tasmota_core::HttpTransport`.
//!
//! Proves the mapping from Tasmota's raw HTTP status to the vendor-neutral
//! `DeviceSnapshot` never fabricates a value: absent energy stays `None` (not a
//! zeroed `Energy`), an unmapped relay label stays `Unknown` (never a guessed
//! `Off`), a non-Tasmota responder is never guessed as Tasmota, and an
//! unreachable host is always `Err`, never a "successful" empty snapshot.
//!
//! All hosts are the httpmock loopback server or an unlisted loopback port
//! (`127.0.0.1:1`, a deterministic connection refusal) - never a real/LAN address.

use httpmock::prelude::*;
use serde_json::json;

use switchkit::{DeviceTarget, Error as SkError, RelayState as SkRelayState, SmartDevice, Vendor};
use tasmota_core::HttpTransport;

fn status0_body(extra: serde_json::Value) -> serde_json::Value {
    let mut base = json!({
        "Status": {"DeviceName": "TestPlug", "Module": 1, "FriendlyName": ["TestPlug"], "Power": 1},
        "StatusFWR": {"Version": "14.2.0"},
        "StatusNET": {"IPAddress": "192.0.2.50", "Mac": "AA:BB:CC:00:11:22", "Hostname": "testplug"},
        "StatusSTS": {"POWER": "ON", "Uptime": "1T02:03:04", "Wifi": {"RSSI": 76}}
    });
    if let (Some(obj), Some(ex)) = (base.as_object_mut(), extra.as_object()) {
        for (k, v) in ex {
            obj.insert(k.clone(), v.clone());
        }
    }
    base
}

fn mock_status(server: &MockServer, body: serde_json::Value) {
    server.mock(|when, then| {
        when.method(GET).path("/cm").query_param("cmnd", "Status 0");
        then.status(200).json_body(body);
    });
}

/// Answer the `StateText` follow-up with the default labels so a plain `ON`/`OFF`
/// relay string normalizes (an unanswered `StateText` yields `Unknown` by design,
/// which is exercised separately by `unmapped_relay_text_is_unknown_not_guessed`).
fn mock_statetext(server: &MockServer) {
    server.mock(|when, then| {
        when.method(GET)
            .path("/cm")
            .query_param("cmnd", "StateText");
        then.status(200)
            .json_body(json!({"StateText1": "OFF", "StateText2": "ON"}));
    });
}

#[tokio::test]
async fn metering_plug_status_maps_every_field_honestly() {
    let server = MockServer::start();
    mock_status(
        &server,
        status0_body(
            json!({"StatusSNS": {"ENERGY": {"Power": 42.5, "Voltage": 230.0, "Total": 12.3}}}),
        ),
    );
    mock_statetext(&server);

    let transport = HttpTransport::default();
    let target = DeviceTarget::new(server.address().to_string());
    let snapshot = transport.status(&target).await.expect("status succeeds");

    assert_eq!(snapshot.relays[0].state, SkRelayState::On);
    assert_eq!(snapshot.energy.as_ref().unwrap().power_w, Some(42.5));
    assert_eq!(snapshot.signal.unwrap().quality_percent, Some(76));
    assert!(snapshot.capabilities.metering);
    assert_eq!(
        snapshot.firmware.unwrap().version,
        Some("14.2.0".to_string())
    );
}

#[tokio::test]
async fn no_energy_block_yields_none_not_zeroed_energy() {
    let server = MockServer::start();
    mock_status(&server, status0_body(json!({})));
    mock_statetext(&server);

    let transport = HttpTransport::default();
    let target = DeviceTarget::new(server.address().to_string());
    let snapshot = transport.status(&target).await.expect("status succeeds");

    assert!(
        snapshot.energy.is_none(),
        "a device with no ENERGY block must yield None, not a zeroed Energy"
    );
    assert!(!snapshot.capabilities.metering);
}

#[tokio::test]
async fn unmapped_relay_text_is_unknown_not_guessed() {
    let server = MockServer::start();
    mock_status(
        &server,
        status0_body(
            json!({"StatusSTS": {"POWER": "Blinking", "Uptime": "1T02:03:04", "Wifi": {"RSSI": 76}}}),
        ),
    );
    mock_statetext(&server);

    let transport = HttpTransport::default();
    let target = DeviceTarget::new(server.address().to_string());
    let snapshot = transport.status(&target).await.expect("status succeeds");

    assert_eq!(
        snapshot.relays[0].state,
        SkRelayState::Unknown("Blinking".to_string()),
        "a relay label that matches neither ON nor OFF must never be guessed Off"
    );
}

#[tokio::test]
async fn probe_confirms_a_tasmota_responder() {
    let server = MockServer::start();
    mock_status(&server, status0_body(json!({})));
    mock_statetext(&server);

    let transport = HttpTransport::default();
    let target = DeviceTarget::new(server.address().to_string());
    let result = transport.probe(&target).await.expect("probe succeeds");

    assert!(result.is_some(), "a Tasmota responder must be confirmed");
}

#[tokio::test]
async fn probe_declines_a_non_tasmota_json_responder() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/cm").query_param("cmnd", "Status 0");
        then.status(200).json_body(json!({"hello": "world"}));
    });

    let transport = HttpTransport::default();
    let target = DeviceTarget::new(server.address().to_string());
    let result = transport
        .probe(&target)
        .await
        .expect("probe must not error on a reachable non-match");

    assert!(
        result.is_none(),
        "a non-Tasmota JSON body must never be guessed as Tasmota"
    );
}

#[tokio::test]
async fn connection_refused_is_network_error_never_a_snapshot() {
    let transport = HttpTransport::default();
    // Port 1 is unlisted on loopback: a fast, deterministic connection refusal,
    // not a real/LAN device.
    let target = DeviceTarget::new("127.0.0.1:1");
    let err = transport
        .status(&target)
        .await
        .expect_err("an unreachable host must error, never a successful empty snapshot");

    assert!(
        matches!(err, SkError::Network { .. }),
        "expected Error::Network, got {err:?}"
    );
}

/// Proves the `switchkit::guardrail` Tasmota table (a deliberate copy, not a
/// delegation, to avoid a `switchkit -> tasmota-core` dependency cycle) stayed in
/// sync with `tasmota_core::guardrail`, the production classifier the CLI's
/// confirmation prompts actually call.
#[test]
fn tasmota_guardrail_parity_with_switchkit() {
    for cmd in [
        "Reset 1",
        "Status 0",
        "SetOption65 1",
        "Backlog Reset; Power ON",
    ] {
        let core_hazard = tasmota_core::guardrail::classify(cmd);
        let sk_hazard = switchkit::guardrail::classify(Vendor::Tasmota, cmd);
        assert_eq!(
            hazard_level(&core_hazard),
            sk_level(&sk_hazard),
            "hazard level mismatch for `{cmd}`: core={core_hazard:?} switchkit={sk_hazard:?}"
        );
    }
}

fn hazard_level(h: &tasmota_core::guardrail::Hazard) -> &'static str {
    match h {
        tasmota_core::guardrail::Hazard::Destructive(_) => "destructive",
        tasmota_core::guardrail::Hazard::RequiresConfirmation => "requires_confirmation",
        tasmota_core::guardrail::Hazard::Safe => "safe",
    }
}

fn sk_level(h: &switchkit::guardrail::Hazard) -> &'static str {
    match h {
        switchkit::guardrail::Hazard::Destructive(_) => "destructive",
        switchkit::guardrail::Hazard::RequiresConfirmation => "requires_confirmation",
        switchkit::guardrail::Hazard::Safe => "safe",
    }
}
