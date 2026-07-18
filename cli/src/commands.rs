//! Command handlers. Each resolves the target selection, performs the work over
//! `tasmota-core`, and returns a rendered [`Output`]. Destructive and bulk writes
//! pass through [`gate`] for confirmation / `--dry-run`.

use std::io::Write;
use std::path::PathBuf;

use serde_json::{Value, json};

use tasmota_core::cache::DeviceCache;
use tasmota_core::guardrail::{self, Hazard};
use tasmota_core::ops::{self, PowerAction};
use tasmota_core::{DeviceStatus, Error, HttpTransport, Result};

use crate::target::{self, Resolved, Selector};
use crate::{Ctx, Output, OutputFormat, render};

/// Per-device outcome carried through [`finish`].
struct PerDevice {
    label: String,
    host: String,
    value: Value,
    text: String,
    error: Option<Error>,
}

/// Prompt on stderr and read a yes/no answer from stdin.
fn confirm(prompt: &str) -> Result<bool> {
    eprint!("{prompt} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    let n = std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| Error::Io {
            message: format!("reading confirmation: {e}"),
        })?;
    if n == 0 {
        // EOF (non-interactive stdin): terminate the prompt line so a following
        // error envelope stays on its own last line of stderr.
        eprintln!();
    }
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// A `--group`/`--all` selection is a bulk write even if it resolves to one device,
/// and always confirms (unless `--yes`).
fn is_bulk(sel: &Selector) -> bool {
    sel.group.is_some() || sel.all
}

/// The outcome of the gate: proceed with the action, or stop and return the
/// (format-aware) dry-run plan.
enum Gate {
    Proceed,
    Stop(Output),
}

/// The confirmation / dry-run gate for a write. Returns `Gate::Stop(plan)` under
/// `--dry-run`, `Gate::Proceed` when cleared, or `Err(Aborted)` when declined. An
/// action that requires confirmation, is a bulk selection, or hits more than one
/// device prompts unless `--yes` is set.
fn gate(
    ctx: &Ctx,
    action: &str,
    targets: &[Resolved],
    require_confirm: bool,
    bulk: bool,
) -> Result<Gate> {
    if ctx.dry_run {
        return Ok(Gate::Stop(plan(ctx, targets, action)));
    }
    if !ctx.yes && (require_confirm || bulk || targets.len() > 1) {
        eprintln!("About to {action} on {} device(s):", targets.len());
        for t in targets {
            eprintln!("  - {} ({})", t.label, t.addr.host);
        }
        if !confirm("Proceed?")? {
            return Err(Error::Aborted {
                message: "aborted by user".into(),
            });
        }
    }
    Ok(Gate::Proceed)
}

fn indent(s: &str) -> String {
    s.lines()
        .map(|l| format!("  {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Turn per-device outcomes into a final [`Output`]. A single target surfaces its
/// error via `Err` (so the CLI emits an error envelope); multiple targets report
/// each outcome and set the exit code to the most severe failure.
fn finish(ctx: &Ctx, mut results: Vec<PerDevice>) -> Result<Output> {
    if results.len() == 1 {
        let r = results.pop().unwrap();
        if let Some(e) = r.error {
            return Err(e);
        }
        return Ok(Output::One(match ctx.format {
            OutputFormat::Json => serde_json::to_string_pretty(&r.value).expect("json"),
            OutputFormat::Text => r.text,
        }));
    }

    let mut exit = 0;
    for r in &results {
        if let Some(e) = &r.error {
            exit = exit.max(e.exit_code());
        }
    }

    let rendered = match ctx.format {
        OutputFormat::Json => {
            let arr: Vec<Value> = results
                .iter()
                .map(|r| match &r.error {
                    Some(e) => json!({
                        "name": r.label, "host": r.host,
                        "error": {"kind": e.kind(), "message": e.to_string()}
                    }),
                    None => json!({"name": r.label, "host": r.host, "result": r.value}),
                })
                .collect();
            serde_json::to_string_pretty(&arr).expect("json")
        }
        OutputFormat::Text => {
            let mut blocks = Vec::new();
            for r in &results {
                match &r.error {
                    Some(e) => blocks.push(format!("{}: error: {e}", r.label)),
                    None => blocks.push(format!("{}:\n{}", r.label, indent(&r.text))),
                }
            }
            blocks.join("\n\n")
        }
    };
    Ok(Output::Many {
        rendered,
        exit_code: exit,
    })
}

fn resolve(ctx: &Ctx, sel: &Selector) -> Result<Vec<Resolved>> {
    target::resolve(sel, &ctx.cache_path, &ctx.groups_path, &ctx.credentials)
}

/// Run a read-only op over every target and collect outcomes.
fn read_each<F>(ctx: &Ctx, targets: Vec<Resolved>, op: F) -> Result<Output>
where
    F: Fn(&HttpTransport, &Resolved) -> Result<(Value, String)>,
{
    let t = ctx.transport();
    let results = targets
        .iter()
        .map(|r| match op(&t, r) {
            Ok((value, text)) => PerDevice {
                label: r.label.clone(),
                host: r.addr.host.clone(),
                value,
                text,
                error: None,
            },
            Err(e) => PerDevice {
                label: r.label.clone(),
                host: r.addr.host.clone(),
                value: Value::Null,
                text: String::new(),
                error: Some(e),
            },
        })
        .collect();
    finish(ctx, results)
}

/// A `--dry-run` plan: the intended action and resolved targets, with no I/O.
/// Format-aware so `--json` still emits structured output for dry-runs.
fn plan(ctx: &Ctx, targets: &[Resolved], action: &str) -> Output {
    match ctx.format {
        OutputFormat::Json => {
            let tgts: Vec<Value> = targets
                .iter()
                .map(|t| json!({"name": t.label, "host": t.addr.host}))
                .collect();
            Output::One(
                serde_json::to_string_pretty(&json!({
                    "dry_run": true,
                    "action": action,
                    "targets": tgts,
                }))
                .expect("json"),
            )
        }
        OutputFormat::Text => {
            let mut s = format!("[dry-run] would {action} on {} device(s):", targets.len());
            for t in targets {
                s.push_str(&format!("\n  - {} ({})", t.label, t.addr.host));
            }
            Output::One(s)
        }
    }
}

/// Resolve targets and run a read op over each, honoring `--dry-run` (which prints
/// the plan and contacts nothing).
fn read_cmd<F>(ctx: &Ctx, sel: &Selector, action: &str, op: F) -> Result<Output>
where
    F: Fn(&HttpTransport, &Resolved) -> Result<(Value, String)>,
{
    let targets = resolve(ctx, sel)?;
    if ctx.dry_run {
        return Ok(plan(ctx, &targets, action));
    }
    read_each(ctx, targets, op)
}

// ---------------------------------------------------------------------------
// Read commands
// ---------------------------------------------------------------------------

pub fn status(ctx: &Ctx, sel: &Selector) -> Result<Output> {
    read_cmd(ctx, sel, "query status", |t, r| {
        let s = ops::get_status(t, &r.addr)?;
        Ok((to_value(&s), render::status(&s)))
    })
}

pub fn power(ctx: &Ctx, sel: &Selector) -> Result<Output> {
    read_cmd(ctx, sel, "read power", |t, r| {
        let s = ops::get_status(t, &r.addr)?;
        let power_w = s.energy.as_ref().and_then(|e| e.power_w);
        Ok((json!({"power_w": power_w}), render::power(&s)))
    })
}

pub fn energy(ctx: &Ctx, sel: &Selector) -> Result<Output> {
    read_cmd(ctx, sel, "read energy", |t, r| {
        let s = ops::get_status(t, &r.addr)?;
        Ok((json!({"energy": s.energy}), render::energy(&s)))
    })
}

pub fn health(ctx: &Ctx, sel: &Selector) -> Result<Output> {
    read_cmd(ctx, sel, "check health", |t, r| {
        let mut s = ops::get_status(t, &r.addr)?;
        // MQTT config/health lives in Status 6; prefer it, keep Status 0's block on a
        // genuine no-answer. A command-level rejection/auth/parse error propagates.
        match ops::mqtt_info(t, &r.addr) {
            Ok(Some(mqtt)) => s.mqtt = Some(mqtt),
            Ok(None) => {}
            Err(Error::Network { .. }) => {}
            Err(e) => return Err(e),
        }
        let v = json!({
            "online": true,
            "rssi": s.wifi_rssi,
            "uptime": s.uptime,
            "firmware": s.firmware,
            "mqtt": s.mqtt,
        });
        Ok((v, render::health(&s)))
    })
}

pub fn devices(ctx: &Ctx) -> Result<Output> {
    let cache = DeviceCache::load(&ctx.cache_path)?;
    Ok(Output::One(match ctx.format {
        OutputFormat::Json => serde_json::to_string_pretty(&cache.devices).expect("json"),
        OutputFormat::Text => render::devices(&cache.devices),
    }))
}

pub fn discover(ctx: &Ctx, range: Option<String>, concurrency: usize) -> Result<Output> {
    let cidr = match range {
        Some(r) => r,
        None => tasmota_core::discovery::detect_local_cidr().ok_or_else(|| Error::Usage {
            message: "could not detect a local subnet; pass --range <CIDR>".into(),
        })?,
    };
    let hosts = tasmota_core::discovery::hosts_in_cidr(&cidr)?;
    if ctx.dry_run {
        return Ok(Output::One(match ctx.format {
            OutputFormat::Json => serde_json::to_string_pretty(&json!({
                "dry_run": true,
                "action": "scan and refresh cache",
                "range": cidr,
                "hosts": hosts.len(),
            }))
            .expect("json"),
            OutputFormat::Text => format!(
                "[dry-run] would scan {} ({} hosts) and refresh the cache",
                cidr,
                hosts.len()
            ),
        }));
    }
    let transport = ctx.transport();
    let found =
        tasmota_core::discovery::scan(&transport, &hosts, concurrency, ctx.credentials.as_ref());

    let statuses: Vec<DeviceStatus> = found.iter().map(|d| d.status.clone()).collect();
    DeviceCache::from_statuses(&statuses).save(&ctx.cache_path)?;

    Ok(Output::One(match ctx.format {
        OutputFormat::Json => {
            let arr: Vec<Value> = found
                .iter()
                .map(|d| {
                    json!({
                        "name": d.status.display_name(),
                        "host": d.host,
                        "firmware": d.status.firmware,
                    })
                })
                .collect();
            serde_json::to_string_pretty(&arr).expect("json")
        }
        OutputFormat::Text => render::discovered(&found),
    }))
}

// ---------------------------------------------------------------------------
// Control commands
// ---------------------------------------------------------------------------

fn relay_desc(relay: Option<u8>) -> String {
    match relay {
        None | Some(0) => "power".to_string(),
        Some(n) => format!("power{n}"),
    }
}

pub fn set_power(
    ctx: &Ctx,
    sel: &Selector,
    relay: Option<u8>,
    action: PowerAction,
    verb: &str,
) -> Result<Output> {
    let targets = resolve(ctx, sel)?;
    if let Gate::Stop(out) = gate(
        ctx,
        &format!("{verb} {}", relay_desc(relay)),
        &targets,
        false,
        is_bulk(sel),
    )? {
        return Ok(out);
    }
    let t = ctx.transport();
    let results = targets
        .iter()
        .map(|r| match ops::set_power(&t, &r.addr, relay, action) {
            Ok(state) => PerDevice {
                label: r.label.clone(),
                host: r.addr.host.clone(),
                value: json!({"relay": state.index, "state": state.state.as_str()}),
                text: format!(
                    "{}: {} -> {}",
                    r.label,
                    relay_desc(relay),
                    state.state.as_str()
                ),
                error: None,
            },
            Err(e) => PerDevice {
                label: r.label.clone(),
                host: r.addr.host.clone(),
                value: Value::Null,
                text: String::new(),
                error: Some(e),
            },
        })
        .collect();
    finish(ctx, results)
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// A setting must be a single command word, never a multi-word command or a
/// `Backlog`. This stops `config get "Backlog Power OFF; Reset 1"` from smuggling a
/// destructive command past the console guardrails; use `console` (which is
/// classified and guarded) for anything with arguments.
fn validate_setting(setting: &str) -> Result<()> {
    if setting.is_empty() || setting.chars().any(|c| c.is_whitespace() || c == ';') {
        return Err(Error::Usage {
            message: format!(
                "`{setting}` is not a single setting name; use `console` (guarded) for \
                 commands with arguments or Backlog"
            ),
        });
    }
    if let Hazard::Destructive(reason) = guardrail::classify(setting) {
        return Err(Error::Usage {
            message: format!(
                "refusing `config` on a destructive command ({reason}); use `console`, \
                 which guards it"
            ),
        });
    }
    Ok(())
}

pub fn config_get(ctx: &Ctx, sel: &Selector, setting: &str) -> Result<Output> {
    validate_setting(setting)?;
    let setting = setting.to_string();
    read_cmd(ctx, sel, &format!("read setting {setting}"), move |t, r| {
        let v = ops::config_get(t, &r.addr, &setting)?;
        Ok((v.clone(), format!("{}: {}", r.label, compact(&v))))
    })
}

pub fn config_set(ctx: &Ctx, sel: &Selector, setting: &str, value: &str) -> Result<Output> {
    validate_setting(setting)?;
    let targets = resolve(ctx, sel)?;
    if let Gate::Stop(out) = gate(
        ctx,
        &format!("set {setting} {value}"),
        &targets,
        true,
        is_bulk(sel),
    )? {
        return Ok(out);
    }
    let t = ctx.transport();
    let results = targets
        .iter()
        .map(|r| match ops::config_set(&t, &r.addr, setting, value) {
            Ok(v) => PerDevice {
                label: r.label.clone(),
                host: r.addr.host.clone(),
                value: v.clone(),
                text: format!("{}: {}", r.label, compact(&v)),
                error: None,
            },
            Err(e) => PerDevice {
                label: r.label.clone(),
                host: r.addr.host.clone(),
                value: Value::Null,
                text: String::new(),
                error: Some(e),
            },
        })
        .collect();
    finish(ctx, results)
}

// ---------------------------------------------------------------------------
// Backup / restore
// ---------------------------------------------------------------------------

fn sanitize(label: &str) -> String {
    label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

pub fn backup(ctx: &Ctx, sel: &Selector, out: Option<PathBuf>) -> Result<Output> {
    let targets = resolve(ctx, sel)?;
    if out.is_some() && targets.len() > 1 {
        return Err(Error::Usage {
            message: "--out cannot be used with multiple targets".into(),
        });
    }
    if ctx.dry_run {
        return Ok(plan(ctx, &targets, "download config backup"));
    }
    let t = ctx.transport();
    let results = targets
        .iter()
        .map(|r| {
            let path = out
                .clone()
                .unwrap_or_else(|| PathBuf::from(format!("{}.dmp", sanitize(&r.label))));
            match ops::backup_config(&t, &r.addr).and_then(|bytes| {
                std::fs::write(&path, &bytes)
                    .map(|_| (bytes.len(), path.clone()))
                    .map_err(|e| Error::Io {
                        message: format!("writing {}: {e}", path.display()),
                    })
            }) {
                Ok((n, p)) => PerDevice {
                    label: r.label.clone(),
                    host: r.addr.host.clone(),
                    value: json!({"file": p.display().to_string(), "bytes": n}),
                    text: format!("{}: wrote {} ({} bytes)", r.label, p.display(), n),
                    error: None,
                },
                Err(e) => PerDevice {
                    label: r.label.clone(),
                    host: r.addr.host.clone(),
                    value: Value::Null,
                    text: String::new(),
                    error: Some(e),
                },
            }
        })
        .collect();
    finish(ctx, results)
}

pub fn restore(ctx: &Ctx, sel: &Selector, file: PathBuf) -> Result<Output> {
    let targets = resolve(ctx, sel)?;
    if ctx.dry_run {
        return Ok(plan(
            ctx,
            &targets,
            &format!("restore config from {}", file.display()),
        ));
    }
    // Validate the file is readable so a real run fails predictably.
    let _data = std::fs::read(&file).map_err(|e| Error::Io {
        message: format!("reading {}: {e}", file.display()),
    })?;
    // The upload endpoint and, crucially, a way to confirm the device actually
    // applied the config are unverified against a live device (see the design
    // spec's open items). We refuse rather than perform an unverified mutation that
    // could report a false success. The `tasmota-core` upload plumbing is in place
    // for when the endpoint is verified.
    Err(Error::Unavailable {
        message: "config restore is not enabled: its upload endpoint and success \
                  criterion are unverified against a live device. Use the device web UI \
                  (Configuration > Backup/Restore) to restore a .dmp for now."
            .into(),
    })
}

// ---------------------------------------------------------------------------
// Firmware
// ---------------------------------------------------------------------------

pub fn firmware_check(ctx: &Ctx, sel: &Selector) -> Result<Output> {
    read_cmd(ctx, sel, "check firmware version", |t, r| {
        let version = ops::firmware_version(t, &r.addr)?;
        Ok((
            json!({"firmware": version}),
            format!("{}: {}", r.label, version),
        ))
    })
}

pub fn firmware_update(ctx: &Ctx, sel: &Selector, url: Option<String>) -> Result<Output> {
    let targets = resolve(ctx, sel)?;
    let action = match &url {
        Some(u) => format!("flash firmware from {u}"),
        None => "flash firmware from the device's OTA URL".to_string(),
    };
    // Dry-run must not touch the device: return the plan and stop before any I/O,
    // including the pre-check below.
    if ctx.dry_run {
        return Ok(plan(ctx, &targets, &action));
    }
    // Pre-check reachability and show current version before flashing. Best-effort:
    // one unreachable device must not abort the whole batch (the per-device update
    // outcomes below carry the failures).
    let t = ctx.transport();
    for r in &targets {
        match ops::firmware_version(&t, &r.addr) {
            Ok(v) => eprintln!("{} ({}): current firmware {}", r.label, r.addr.host, v),
            Err(e) => eprintln!("{} ({}): unreachable ({e})", r.label, r.addr.host),
        }
    }
    if let Gate::Stop(out) = gate(ctx, &action, &targets, true, is_bulk(sel))? {
        return Ok(out);
    }
    let results = targets
        .iter()
        .map(
            |r| match ops::firmware_update(&t, &r.addr, url.as_deref()) {
                Ok(_) => PerDevice {
                    label: r.label.clone(),
                    host: r.addr.host.clone(),
                    value: json!({"upgrade": "started"}),
                    text: format!("{}: upgrade started", r.label),
                    error: None,
                },
                Err(e) => PerDevice {
                    label: r.label.clone(),
                    host: r.addr.host.clone(),
                    value: Value::Null,
                    text: String::new(),
                    error: Some(e),
                },
            },
        )
        .collect();
    finish(ctx, results)
}

// ---------------------------------------------------------------------------
// Console (guardrail-classified)
// ---------------------------------------------------------------------------

pub fn console(ctx: &Ctx, sel: &Selector, command: &str) -> Result<Output> {
    let targets = resolve(ctx, sel)?;
    // A destructive or unclassifiable command must be confirmed even for a single
    // target; only known-safe read/control commands skip the single-target prompt.
    let require_confirm = match guardrail::classify(command) {
        Hazard::Destructive(reason) => {
            eprintln!("This command is destructive: {reason}");
            true
        }
        Hazard::RequiresConfirmation => {
            eprintln!("`{command}` is not a known-safe command; confirmation required.");
            true
        }
        Hazard::Safe => false,
    };
    if let Gate::Stop(out) = gate(
        ctx,
        &format!("send `{command}`"),
        &targets,
        require_confirm,
        is_bulk(sel),
    )? {
        return Ok(out);
    }
    let cmd = command.to_string();
    read_each(ctx, targets, move |t, r| {
        let v = ops::console(t, &r.addr, &cmd)?;
        Ok((v.clone(), format!("{}: {}", r.label, compact(&v))))
    })
}

// ---------------------------------------------------------------------------
// Template
// ---------------------------------------------------------------------------

pub fn template_get(ctx: &Ctx, sel: &Selector) -> Result<Output> {
    read_cmd(ctx, sel, "read template", |t, r| {
        let v = ops::template_get(t, &r.addr)?;
        Ok((v.clone(), format!("{}: {}", r.label, compact(&v))))
    })
}

pub fn template_apply(ctx: &Ctx, sel: &Selector, json_str: &str) -> Result<Output> {
    let targets = resolve(ctx, sel)?;
    if let Gate::Stop(out) = gate(ctx, "apply GPIO template", &targets, true, is_bulk(sel))? {
        return Ok(out);
    }
    let t = ctx.transport();
    let results = targets
        .iter()
        .map(|r| match ops::template_apply(&t, &r.addr, json_str) {
            Ok(v) => PerDevice {
                label: r.label.clone(),
                host: r.addr.host.clone(),
                value: v.clone(),
                text: format!("{}: template applied", r.label),
                error: None,
            },
            Err(e) => PerDevice {
                label: r.label.clone(),
                host: r.addr.host.clone(),
                value: Value::Null,
                text: String::new(),
                error: Some(e),
            },
        })
        .collect();
    finish(ctx, results)
}

// ---------------------------------------------------------------------------
// Groups
// ---------------------------------------------------------------------------

pub fn group_list(ctx: &Ctx) -> Result<Output> {
    let groups = target::list_groups(&ctx.groups_path)?;
    Ok(Output::One(match ctx.format {
        OutputFormat::Json => serde_json::to_string_pretty(&groups).expect("json"),
        OutputFormat::Text => {
            if groups.is_empty() {
                format!("no groups defined in {}", ctx.groups_path.display())
            } else {
                groups
                    .iter()
                    .map(|(name, members)| format!("{name}: {}", members.join(", ")))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
    }))
}

// ---------------------------------------------------------------------------
// Watch
// ---------------------------------------------------------------------------

pub fn watch(ctx: &Ctx, sel: &Selector, interval: u64) -> Result<Output> {
    let targets = resolve(ctx, sel)?;
    if ctx.dry_run {
        return Ok(plan(ctx, &targets, "watch status"));
    }
    let t = ctx.transport();
    loop {
        // Clear screen and move cursor home.
        print!("\x1b[2J\x1b[H");
        for r in &targets {
            match ops::get_status(&t, &r.addr) {
                Ok(s) => println!("{}\n", render::status(&s)),
                Err(e) => println!("{}: error: {e}\n", r.label),
            }
        }
        let _ = std::io::stdout().flush();
        std::thread::sleep(std::time::Duration::from_secs(interval.max(1)));
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn to_value(s: &DeviceStatus) -> Value {
    serde_json::to_value(s).expect("DeviceStatus serializes")
}

/// Compact one-line JSON for embedding a device response in a text line.
fn compact(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "<unserializable>".into())
}
