# tasmota

An unofficial, HTTP-first command-line tool for managing [Tasmota](https://tasmota.github.io/docs/)
smart devices: control relays, read energy, back up and restore config, flash
firmware, run console commands, and discover devices on your network.

> This is a third-party tool. It is not affiliated with the Tasmota project.

It is a sibling to the `shelly` CLI and follows [The CLI Spec](https://clispec.dev):
text on a TTY, JSON when piped, a `schema` subcommand, and structured error
envelopes. It talks directly to each device over HTTP (no MQTT broker required).

## Install

```sh
cargo install tasmota-cli      # installs the `tasmota` binary
```

## Usage

Devices are selected with one of `--host <ip>`, `--name <cached>`, `--group <name>`,
or `--all`. Discover devices first to populate the name cache:

```sh
tasmota discover                       # scan the local /24, cache what's found
tasmota discover --range 192.0.2.0/24  # scan an explicit range

tasmota --name Dryer status            # full status
tasmota --host 192.0.2.50 on           # turn the primary relay on
tasmota --name Freezer power           # instantaneous watts
tasmota --name Freezer energy          # kWh totals
tasmota --all health                   # health across every cached device

tasmota --name Plug console "Status 8" # arbitrary command
tasmota --name Plug backup             # download the .dmp config
```

Output is human-readable on a terminal and JSON when piped:

```sh
tasmota --name Freezer power -o json
{ "power_w": 42.5 }
```

### Authentication

For devices with a web password, pass `--user`/`--password` or set `TASMOTA_USER`
and `TASMOTA_PASSWORD`. Credentials are never persisted to the device cache.

## Safety

Destructive operations - `firmware update`, `config restore`, `config set`,
`template apply`, and any destructive `console` command (e.g. `Reset`, `Upgrade`) -
require confirmation. Use `--dry-run` to preview the plan, or `--yes` to skip the
prompt in scripts. Bulk operations (`--group`/`--all`) also confirm before writing.

Absent data is reported as `n/a`/`null`, never `0`: a plug with no energy sensor, an
offline device, and a genuine zero reading are three different facts.

## Development

```sh
make check   # fmt --check, clippy -D warnings, tests
make test
make lint
```

Two crates: `tasmota-core` (the I/O-agnostic library: transport, `Status 0`
parsing, discovery, guardrails) and `tasmota-cli` (this CLI). CI runs the same
`make` targets.

## License

MIT
