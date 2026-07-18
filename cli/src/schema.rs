//! The clispec v0.2 contract emitted by `tasmota schema`.
//!
//! Conforms to <https://clispec.dev/schema/v0.2.json> (validated by
//! `tests/conformance.rs` against the vendored copy). Keep in sync with the clap
//! definitions in `main.rs` and the error kinds in `tasmota_core::Error`.

use serde_json::{Value, json};

pub const CLISPEC_VERSION: &str = "0.2";

/// Build the clispec contract as a JSON value.
pub fn contract() -> Value {
    json!({
        "clispec": CLISPEC_VERSION,
        "name": "tasmota",
        "version": env!("CARGO_PKG_VERSION"),
        "description": env!("CARGO_PKG_DESCRIPTION"),
        "global_args": [
            {"name": "--output", "type": "string", "enum": ["auto", "json", "text"], "default": "auto",
             "description": "Output format. auto = text on a TTY, JSON when piped."},
            {"name": "--host", "type": "string",
             "description": "Target a device by IP address or hostname."},
            {"name": "--name", "type": "string",
             "description": "Target a device by cached name (see `discover`)."},
            {"name": "--group", "type": "string",
             "description": "Target a named group from groups.toml."},
            {"name": "--all", "type": "boolean", "default": false,
             "description": "Target every cached device."},
            {"name": "--user", "type": "string",
             "description": "Web username; overrides TASMOTA_USER."},
            {"name": "--password", "type": "string",
             "description": "Web password; overrides TASMOTA_PASSWORD."},
            {"name": "--timeout", "type": "integer", "default": 5,
             "description": "Per-request timeout in seconds."},
            {"name": "--yes", "type": "boolean", "default": false,
             "description": "Skip confirmation prompts for destructive/bulk operations."},
            {"name": "--dry-run", "type": "boolean", "default": false,
             "description": "Resolve targets and print the intended action without sending it."}
        ],
        "commands": [
            {"name": "discover", "mutating": true, "stability": "stable",
             "description": "Scan the network for Tasmota devices and overwrite the local device cache.",
             "args": [
                {"name": "--range", "type": "string", "description": "CIDR to scan (default: derived local /24)."},
                {"name": "--concurrency", "type": "integer", "default": 64, "description": "Max in-flight probes."}
             ],
             "output_fields": [
                {"name": "name", "type": "string"},
                {"name": "host", "type": "string"},
                {"name": "firmware", "type": "string"}
             ]},
            {"name": "devices", "mutating": false, "stability": "stable",
             "description": "List cached devices."},
            {"name": "status", "mutating": false, "stability": "stable",
             "description": "Show full device status. Targets the device selected by the global --host/--name/--group/--all options.",
             "output_fields": [
                {"name": "name", "type": "string"},
                {"name": "relays", "type": "object[]"},
                {"name": "firmware", "type": "string"},
                {"name": "energy", "type": "object", "description": "null when the device has no energy sensor."}
             ]},
            {"name": "on", "mutating": true, "stability": "stable",
             "description": "Turn a relay on. Targets the device selected by the global --host/--name/--group/--all options.",
             "args": [{"name": "--relay", "type": "integer", "description": "Relay index (default: primary)."}]},
            {"name": "off", "mutating": true, "stability": "stable",
             "description": "Turn a relay off. Targets the device selected by the global --host/--name/--group/--all options.",
             "args": [{"name": "--relay", "type": "integer", "description": "Relay index (default: primary)."}]},
            {"name": "toggle", "mutating": true, "stability": "stable",
             "description": "Toggle a relay. Targets the device selected by the global --host/--name/--group/--all options.",
             "args": [{"name": "--relay", "type": "integer", "description": "Relay index (default: primary)."}]},
            {"name": "switch", "mutating": true, "stability": "stable",
             "description": "Set a relay to an explicit state. Targets the device selected by the global --host/--name/--group/--all options.",
             "args": [
                {"name": "state", "type": "string", "required": true, "enum": ["on", "off", "toggle"], "description": "Target relay state."},
                {"name": "--relay", "type": "integer", "description": "Relay index (default: primary)."}
             ]},
            {"name": "power", "mutating": false, "stability": "stable",
             "description": "Show instantaneous power (W). Targets the device selected by the global --host/--name/--group/--all options.",
             "output_fields": [{"name": "power_w", "type": "number", "description": "null when the device has no energy sensor."}]},
            {"name": "energy", "mutating": false, "stability": "stable",
             "description": "Show energy totals (kWh). Targets the device selected by the global --host/--name/--group/--all options."},
            {"name": "health", "mutating": false, "stability": "stable",
             "description": "Show device health (online, RSSI, uptime, firmware, MQTT). Targets the device selected by the global --host/--name/--group/--all options."},
            {"name": "config", "mutating": false, "stability": "stable",
             "description": "Read or write device settings.",
             "subcommands": [
                {"name": "get", "mutating": false, "stability": "stable",
                 "description": "Read a setting.",
                 "args": [{"name": "setting", "type": "string", "required": true, "description": "Setting/command name."}]},
                {"name": "set", "mutating": true, "stability": "stable",
                 "description": "Write a setting (guarded).",
                 "args": [
                    {"name": "setting", "type": "string", "required": true},
                    {"name": "value", "type": "string", "required": true}
                 ]}
             ]},
            {"name": "backup", "mutating": true, "stability": "stable",
             "description": "Download the device config backup (.dmp) and write it to a local file.",
             "args": [{"name": "--out", "type": "path", "description": "Output file (default: <name>.dmp)."}]},
            {"name": "restore", "mutating": true, "stability": "experimental",
             "description": "Restore a config backup (.dmp). Guarded; upload endpoint is unverified.",
             "args": [{"name": "file", "type": "path", "required": true}]},
            {"name": "firmware", "mutating": false, "stability": "stable",
             "description": "Check or update firmware.",
             "subcommands": [
                {"name": "check", "mutating": false, "stability": "stable",
                 "description": "Show the installed firmware version."},
                {"name": "update", "mutating": true, "stability": "stable",
                 "description": "Trigger an OTA firmware upgrade (guarded, can brick).",
                 "args": [{"name": "--url", "type": "string", "description": "OTA URL to flash from."}]}
             ]},
            {"name": "console", "mutating": true, "stability": "stable",
             "description": "Send an arbitrary command or Backlog. Destructive (sub)commands are guarded.",
             "args": [{"name": "command", "type": "string", "required": true, "description": "The Tasmota command to send."}]},
            {"name": "template", "mutating": false, "stability": "stable",
             "description": "Read or apply a GPIO template.",
             "subcommands": [
                {"name": "get", "mutating": false, "stability": "stable", "description": "Show the current template."},
                {"name": "apply", "mutating": true, "stability": "stable",
                 "description": "Apply a template and activate it (guarded).",
                 "args": [{"name": "json", "type": "string", "required": true, "description": "Template JSON."}]}
             ]},
            {"name": "group", "mutating": false, "stability": "stable",
             "description": "List device groups defined in groups.toml.",
             "subcommands": [
                {"name": "list", "mutating": false, "stability": "stable", "description": "List groups and members."}
             ]},
            {"name": "watch", "mutating": false, "stability": "beta",
             "description": "Live-updating dashboard of targeted devices.",
             "args": [{"name": "--interval", "type": "integer", "default": 2, "description": "Refresh interval (seconds)."}]},
            {"name": "completions", "mutating": false, "stability": "stable",
             "description": "Generate a shell completion script.",
             "args": [{"name": "shell", "type": "string", "required": true,
                       "enum": ["bash", "zsh", "fish", "powershell", "elvish"]}]},
            {"name": "schema", "mutating": false, "stability": "stable",
             "description": "Print this clispec contract as JSON."}
        ],
        "errors": [
            {"kind": "usage", "exit_code": 3, "retryable": false, "description": "Invalid command-line arguments."},
            {"kind": "not_found", "exit_code": 4, "retryable": false, "description": "Targeted device name not in the cache."},
            {"kind": "auth", "exit_code": 5, "retryable": false, "description": "Device required authentication we lacked or that was rejected."},
            {"kind": "network", "exit_code": 6, "retryable": true, "description": "Device unreachable or transport failed."},
            {"kind": "command_rejected", "exit_code": 7, "retryable": false, "description": "Device returned HTTP 200 but rejected the command in its JSON body."},
            {"kind": "parse", "exit_code": 8, "retryable": false, "description": "Device response was not in the expected shape."},
            {"kind": "io", "exit_code": 1, "retryable": false, "description": "A local file operation failed."},
            {"kind": "unavailable", "exit_code": 9, "retryable": false, "description": "The requested datum is not available from the device (never coerced to 0)."},
            {"kind": "aborted", "exit_code": 2, "retryable": false, "description": "User declined a confirmation prompt."}
        ]
    })
}

/// The contract as a pretty-printed JSON string.
pub fn contract_json() -> String {
    serde_json::to_string_pretty(&contract()).expect("contract serializes")
}
