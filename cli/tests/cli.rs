//! End-to-end tests of the compiled `tasmota` binary against a mocked Tasmota
//! device (httpmock). All addresses are loopback/documentation ranges.

use std::process::Command;

use httpmock::prelude::*;
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_tasmota");

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Run the binary with a specific XDG config dir.
fn run_env(xdg: &std::path::Path, args: &[&str]) -> Output {
    let out = Command::new(BIN)
        .args(args)
        .env("XDG_CONFIG_HOME", xdg)
        .env_remove("TASMOTA_USER")
        .env_remove("TASMOTA_PASSWORD")
        .output()
        .expect("spawn binary");
    Output {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

/// Run the binary with a shared empty XDG config dir (tests that don't need a cache).
fn run(args: &[&str]) -> Output {
    run_env(&std::env::temp_dir().join("tasmota-cli-tests-xdg"), args)
}

fn error_envelope(stderr: &str) -> Value {
    let last = stderr.lines().last().expect("stderr has an error line");
    serde_json::from_str::<Value>(last).expect("error envelope is JSON")["error"].clone()
}

fn status0_body(extra: Value) -> Value {
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

fn mock_status(server: &MockServer, body: Value) {
    server.mock(|when, then| {
        when.method(GET).path("/cm").query_param("cmnd", "Status 0");
        then.status(200).json_body(body);
    });
}

/// Answer the `StateText` follow-up with the default labels so relay state
/// normalizes to on/off (an unanswered StateText yields Unknown by design).
fn mock_statetext(server: &MockServer) {
    server.mock(|when, then| {
        when.method(GET)
            .path("/cm")
            .query_param("cmnd", "StateText");
        then.status(200)
            .json_body(json!({"StateText1": "OFF", "StateText2": "ON"}));
    });
}

#[test]
fn schema_is_clispec_v0_2() {
    let out = run(&["schema"]);
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    let v: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(v["clispec"], "0.2");
    assert_eq!(v["name"], "tasmota");
}

#[test]
fn help_mentions_schema() {
    let out = run(&["--help"]);
    assert_eq!(out.code, 0);
    assert!(out.stdout.contains("schema"));
}

#[test]
fn status_over_http_reports_relay_and_energy() {
    let server = MockServer::start();
    mock_status(
        &server,
        status0_body(json!({"StatusSNS": {"ENERGY": {"Power": 42.5, "Total": 12.3}}})),
    );
    mock_statetext(&server);
    let host = server.address().to_string();
    let out = run(&["-o", "json", "--host", &host, "status"]);
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    let v: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(v["relays"][0]["state"], "on");
    assert_eq!(v["energy"]["power_w"], 42.5);
    assert_eq!(v["firmware"], "14.2.0");
}

#[test]
fn power_without_sensor_is_null_not_zero() {
    let server = MockServer::start();
    // No ENERGY block: the device has no energy sensor.
    mock_status(&server, status0_body(json!({})));
    let host = server.address().to_string();
    let out = run(&["-o", "json", "--host", &host, "power"]);
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    let v: Value = serde_json::from_str(&out.stdout).unwrap();
    assert!(v["power_w"].is_null(), "absent power must be null, not 0");
}

#[test]
fn toggle_with_yes_sends_power_command() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(GET)
            .path("/cm")
            .query_param("cmnd", "Power TOGGLE");
        then.status(200).json_body(json!({"POWER": "OFF"}));
    });
    mock_statetext(&server);
    let host = server.address().to_string();
    let out = run(&["-o", "json", "--host", &host, "--yes", "toggle"]);
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    m.assert();
    let v: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(v["state"], "off");
}

#[test]
fn unknown_name_exits_not_found() {
    let out = run(&["--name", "does-not-exist", "status"]);
    assert_eq!(out.code, 4);
    assert_eq!(error_envelope(&out.stderr)["kind"], "not_found");
}

#[test]
fn no_target_exits_usage() {
    let out = run(&["status"]);
    assert_eq!(out.code, 3);
    assert_eq!(error_envelope(&out.stderr)["kind"], "usage");
}

#[test]
fn command_rejected_maps_to_exit_7() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/cm");
        then.status(200).json_body(json!({"Command": "Unknown"}));
    });
    let host = server.address().to_string();
    // HTTP 200 with an error body must NOT be treated as success.
    let out = run(&["--host", &host, "status"]);
    assert_eq!(out.code, 7, "stderr: {}", out.stderr);
    assert_eq!(error_envelope(&out.stderr)["kind"], "command_rejected");
}

#[test]
fn dry_run_firmware_update_contacts_nothing() {
    let server = MockServer::start();
    let any = server.mock(|when, then| {
        when.method(GET).path("/cm");
        then.status(200)
            .json_body(json!({"StatusFWR": {"Version": "14.2.0"}}));
    });
    let host = server.address().to_string();
    let out = run(&[
        "--host",
        &host,
        "--dry-run",
        "firmware",
        "update",
        "--url",
        "http://example/f.bin",
    ]);
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert_eq!(
        any.hits(),
        0,
        "dry-run firmware update must not contact the device"
    );
}

#[test]
fn dry_run_status_contacts_nothing() {
    let server = MockServer::start();
    let any = server.mock(|when, then| {
        when.method(GET).path("/cm");
        then.status(200)
            .json_body(json!({"StatusFWR": {"Version": "14.2.0"}}));
    });
    let host = server.address().to_string();
    let out = run(&["-o", "json", "--host", &host, "--dry-run", "status"]);
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert_eq!(any.hits(), 0, "dry-run status must not contact the device");
    // Dry-run still honors --json.
    let v: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(v["dry_run"], true);
    assert_eq!(v["action"], "query status");
}

#[test]
fn dry_run_backup_writes_no_file() {
    let server = MockServer::start();
    let any = server.mock(|when, then| {
        when.method(GET).path("/dl");
        then.status(200).body("dmp");
    });
    let host = server.address().to_string();
    let out_path =
        std::env::temp_dir().join(format!("tasmota-dryrun-backup-{}.dmp", server.port()));
    let _ = std::fs::remove_file(&out_path);
    let out = run(&[
        "--host",
        &host,
        "--dry-run",
        "backup",
        "--out",
        out_path.to_str().unwrap(),
    ]);
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert_eq!(any.hits(), 0, "dry-run backup must not download");
    assert!(!out_path.exists(), "dry-run backup must not write a file");
}

#[test]
fn unclassified_console_command_requires_confirmation() {
    let server = MockServer::start();
    let any = server.mock(|when, then| {
        when.method(GET).path("/cm");
        then.status(200).json_body(json!({"SetOption65": "ON"}));
    });
    let host = server.address().to_string();
    // No --yes, no --dry-run: with no stdin, confirmation is declined (EOF), so the
    // command must NOT be sent.
    let out = run(&["--host", &host, "console", "SetOption65 1"]);
    assert_eq!(out.code, 2, "should abort; stderr: {}", out.stderr);
    assert_eq!(any.hits(), 0, "must not send before confirmation");
    assert_eq!(error_envelope(&out.stderr)["kind"], "aborted");
}

#[test]
fn config_get_rejects_command_injection() {
    let server = MockServer::start();
    let any = server.mock(|when, then| {
        when.method(GET).path("/cm");
        then.status(200).json_body(json!({"Command": "Unknown"}));
    });
    let host = server.address().to_string();
    let out = run(&[
        "--host",
        &host,
        "config",
        "get",
        "Backlog Power OFF; Reset 1",
    ]);
    assert_eq!(out.code, 3, "stderr: {}", out.stderr);
    assert_eq!(any.hits(), 0, "must not send an injected command");
    assert_eq!(error_envelope(&out.stderr)["kind"], "usage");
}

#[test]
fn bulk_selection_with_single_device_still_confirms() {
    let server = MockServer::start();
    let any = server.mock(|when, then| {
        when.method(GET).path("/cm");
        then.status(200).json_body(json!({"POWER": "OFF"}));
    });
    let host = server.address().to_string();
    // Isolated XDG config with exactly one cached device.
    let xdg = std::env::temp_dir().join(format!("tasmota-cli-bulk-{}", server.port()));
    let cfg = xdg.join("tasmota");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("devices.json"),
        format!("{{\"devices\":[{{\"name\":\"only\",\"host\":\"{host}\"}}]}}"),
    )
    .unwrap();
    // `--all` is a bulk write even with one device: it must confirm (EOF -> abort).
    let out = run_env(&xdg, &["--all", "off"]);
    assert_eq!(
        out.code, 2,
        "single-device --all must still confirm; stderr: {}",
        out.stderr
    );
    assert_eq!(any.hits(), 0, "must not send before confirmation");
    assert_eq!(error_envelope(&out.stderr)["kind"], "aborted");
}

#[test]
fn switch_sets_explicit_relay_state() {
    let server = MockServer::start();
    let m = server.mock(|when, then| {
        when.method(GET)
            .path("/cm")
            .query_param("cmnd", "Power2 OFF");
        then.status(200).json_body(json!({"POWER2": "OFF"}));
    });
    mock_statetext(&server);
    let host = server.address().to_string();
    let out = run(&[
        "-o", "json", "--host", &host, "--yes", "switch", "off", "--relay", "2",
    ]);
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    m.assert();
    let v: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(v["state"], "off");
    assert_eq!(v["relay"], 2);
}

#[test]
fn restore_is_unavailable_pending_verification() {
    let f = std::env::temp_dir().join("tasmota-cli-test-restore.dmp");
    std::fs::write(&f, b"dummy").unwrap();
    let out = run(&["--host", "192.0.2.99", "restore", f.to_str().unwrap()]);
    assert_eq!(out.code, 9, "stderr: {}", out.stderr);
    assert_eq!(error_envelope(&out.stderr)["kind"], "unavailable");
}

#[test]
fn destructive_console_dry_run_sends_nothing() {
    let server = MockServer::start();
    let any = server.mock(|when, then| {
        when.method(GET).path("/cm");
        then.status(200).json_body(json!({"Command": "Unknown"}));
    });
    let host = server.address().to_string();
    let out = run(&["--host", &host, "--dry-run", "console", "Reset 1"]);
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert_eq!(any.hits(), 0, "dry-run must not contact the device");
    assert!(out.stderr.contains("destructive"));
}
