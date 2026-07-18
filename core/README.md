# tasmota-core

The I/O-agnostic core library behind the [`tasmota`](https://crates.io/crates/tasmota-cli)
CLI (an unofficial tool for Tasmota smart devices).

It models a Tasmota device from its `Status 0` response, talks to devices over
HTTP, discovers them on a network, and classifies destructive commands. It contains
no terminal rendering or process-exit logic, so it can back a CLI or a service
equally.

Guiding rules, enforced by tests:

- Absent data is `None` (never coerced to `0`).
- Live relay state comes from `StatusSTS.POWER`, never the `Status.Power` bitmask.
- HTTP 200 is not success; the JSON body decides.
- No network addresses are hardcoded.

## License

MIT
