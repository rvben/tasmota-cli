//! `tasmota`: an unofficial CLI for Tasmota smart devices.
//!
//! Follows The CLI Spec (clispec.dev): text on a TTY, JSON when piped, a `schema`
//! subcommand, structured error envelopes on the last line of stderr, and
//! `mutating` markers in the schema.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::error::ErrorKind as ClapErrorKind;
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use serde_json::json;

use tasmota_cli::target::Selector;
use tasmota_cli::{Ctx, Output, OutputFormat, commands, config_dir, resolve_credentials, schema};
use tasmota_core::Error;
use tasmota_core::ops::PowerAction;

#[derive(Parser)]
#[command(
    name = "tasmota",
    version,
    about = "Unofficial CLI for managing Tasmota smart devices over HTTP.",
    long_about = "Unofficial CLI for managing Tasmota smart devices over HTTP.\n\nRun `tasmota schema` for the machine-readable contract (clispec.dev).",
    subcommand_required = true,
    arg_required_else_help = true
)]
struct Cli {
    #[command(flatten)]
    global: GlobalArgs,

    #[command(subcommand)]
    command: Command,
}

#[derive(Args, Clone)]
struct GlobalArgs {
    /// Output format; auto = text on a TTY, JSON when piped.
    #[arg(long, short = 'o', value_enum, default_value = "auto", global = true)]
    output: CliOutput,

    /// Target a device by IP address or hostname.
    #[arg(long, global = true)]
    host: Option<String>,

    /// Target a device by cached name.
    #[arg(long, short = 'n', global = true)]
    name: Option<String>,

    /// Target a named group from groups.toml. Format: a [groups] table mapping each
    /// name to a list, e.g. `kitchen = ["Freezer", "Dishwasher", "10.10.20.5"]`.
    #[arg(long, short = 'g', global = true)]
    group: Option<String>,

    /// Target every cached device.
    #[arg(long, global = true)]
    all: bool,

    /// Web username (overrides TASMOTA_USER).
    #[arg(long, global = true)]
    user: Option<String>,

    /// Web password (overrides TASMOTA_PASSWORD).
    #[arg(long, global = true)]
    password: Option<String>,

    /// Per-request timeout in seconds.
    #[arg(long, global = true, default_value_t = 5)]
    timeout: u64,

    /// Skip confirmation prompts for destructive/bulk operations.
    #[arg(long, short = 'y', global = true)]
    yes: bool,

    /// Resolve targets and print the intended action without sending it.
    #[arg(long, global = true)]
    dry_run: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Scan the network for Tasmota devices and refresh the cache.
    Discover {
        /// CIDR to scan (default: derived local /24).
        #[arg(long)]
        range: Option<String>,
        /// Max in-flight probes.
        #[arg(long, default_value_t = 64)]
        concurrency: usize,
    },
    /// List cached devices.
    Devices,
    /// Show full device status.
    Status,
    /// Turn a relay on.
    On {
        #[arg(long)]
        relay: Option<u8>,
    },
    /// Turn a relay off.
    Off {
        #[arg(long)]
        relay: Option<u8>,
    },
    /// Toggle a relay.
    Toggle {
        #[arg(long)]
        relay: Option<u8>,
    },
    /// Set a relay to an explicit state.
    Switch {
        #[arg(value_enum)]
        state: SwitchState,
        #[arg(long)]
        relay: Option<u8>,
    },
    /// Show instantaneous power (W).
    Power,
    /// Show energy totals (kWh).
    Energy,
    /// Show device health.
    Health,
    /// Read or write device settings.
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// Download the device config backup (.dmp).
    Backup {
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Restore a config backup (.dmp). Experimental; endpoint unverified.
    Restore { file: PathBuf },
    /// Check or update firmware.
    Firmware {
        #[command(subcommand)]
        cmd: FirmwareCmd,
    },
    /// Send an arbitrary command or Backlog (destructive commands are guarded).
    Console {
        /// The Tasmota command to send.
        command: String,
    },
    /// Read or apply a GPIO template.
    Template {
        #[command(subcommand)]
        cmd: TemplateCmd,
    },
    /// List device groups defined in groups.toml.
    Group {
        #[command(subcommand)]
        cmd: GroupCmd,
    },
    /// Live-updating dashboard of targeted devices.
    Watch {
        #[arg(long, default_value_t = 2)]
        interval: u64,
    },
    /// Generate a shell completion script.
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Print the clispec contract as JSON.
    Schema,
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Read a setting.
    Get { setting: String },
    /// Write a setting (guarded).
    Set { setting: String, value: String },
}

#[derive(Subcommand)]
enum FirmwareCmd {
    /// Show the installed firmware version.
    Check,
    /// Trigger an OTA firmware upgrade (guarded, can brick).
    Update {
        #[arg(long)]
        url: Option<String>,
    },
}

#[derive(Subcommand)]
enum TemplateCmd {
    /// Show the current template.
    Get,
    /// Apply a template and activate it (guarded).
    Apply { json: String },
}

#[derive(Subcommand)]
enum GroupCmd {
    /// List groups and members.
    List,
}

#[derive(Clone, Copy, ValueEnum)]
enum CliOutput {
    Auto,
    Json,
    Text,
}

#[derive(Clone, Copy, ValueEnum)]
enum SwitchState {
    On,
    Off,
    Toggle,
}

impl From<SwitchState> for PowerAction {
    fn from(s: SwitchState) -> Self {
        match s {
            SwitchState::On => PowerAction::On,
            SwitchState::Off => PowerAction::Off,
            SwitchState::Toggle => PowerAction::Toggle,
        }
    }
}

impl CliOutput {
    fn resolve(self) -> OutputFormat {
        match self {
            CliOutput::Json => OutputFormat::Json,
            CliOutput::Text => OutputFormat::Text,
            CliOutput::Auto => {
                if std::io::stdout().is_terminal() {
                    OutputFormat::Text
                } else {
                    OutputFormat::Json
                }
            }
        }
    }
}

fn build_ctx(g: &GlobalArgs) -> Ctx {
    let dir = config_dir();
    Ctx {
        format: g.output.resolve(),
        timeout_secs: g.timeout,
        credentials: resolve_credentials(g.user.clone(), g.password.clone()),
        cache_path: dir.join("devices.json"),
        groups_path: dir.join("groups.toml"),
        yes: g.yes,
        dry_run: g.dry_run,
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    // Restore default SIGPIPE handling so piping into `head`/`less` exits quietly
    // instead of panicking on a broken pipe (Rust ignores SIGPIPE by default).
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => return handle_clap_error(e),
    };

    // These need neither a device nor a context.
    match &cli.command {
        Command::Schema => {
            println!("{}", schema::contract_json());
            return ExitCode::SUCCESS;
        }
        Command::Completions { shell } => {
            let mut cmd = Cli::command();
            let name = cmd.get_name().to_string();
            clap_complete::generate(*shell, &mut cmd, name, &mut std::io::stdout());
            return ExitCode::SUCCESS;
        }
        _ => {}
    }

    let ctx = build_ctx(&cli.global);
    let sel = Selector {
        host: cli.global.host.clone(),
        name: cli.global.name.clone(),
        group: cli.global.group.clone(),
        all: cli.global.all,
    };

    match dispatch(&cli.command, &ctx, &sel).await {
        Ok(output) => emit(output),
        Err(err) => {
            emit_error(&err);
            ExitCode::from(err.exit_code() as u8)
        }
    }
}

async fn dispatch(command: &Command, ctx: &Ctx, sel: &Selector) -> tasmota_core::Result<Output> {
    match command {
        Command::Discover { range, concurrency } => {
            commands::discover(ctx, range.clone(), *concurrency).await
        }
        Command::Devices => commands::devices(ctx),
        Command::Status => commands::status(ctx, sel).await,
        Command::On { relay } => {
            commands::set_power(ctx, sel, *relay, PowerAction::On, "turn on").await
        }
        Command::Off { relay } => {
            commands::set_power(ctx, sel, *relay, PowerAction::Off, "turn off").await
        }
        Command::Toggle { relay } => {
            commands::set_power(ctx, sel, *relay, PowerAction::Toggle, "toggle").await
        }
        Command::Switch { state, relay } => {
            commands::set_power(ctx, sel, *relay, (*state).into(), "switch").await
        }
        Command::Power => commands::power(ctx, sel).await,
        Command::Energy => commands::energy(ctx, sel).await,
        Command::Health => commands::health(ctx, sel).await,
        Command::Config { cmd } => match cmd {
            ConfigCmd::Get { setting } => commands::config_get(ctx, sel, setting).await,
            ConfigCmd::Set { setting, value } => {
                commands::config_set(ctx, sel, setting, value).await
            }
        },
        Command::Backup { out } => commands::backup(ctx, sel, out.clone()).await,
        Command::Restore { file } => commands::restore(ctx, sel, file.clone()),
        Command::Firmware { cmd } => match cmd {
            FirmwareCmd::Check => commands::firmware_check(ctx, sel).await,
            FirmwareCmd::Update { url } => commands::firmware_update(ctx, sel, url.clone()).await,
        },
        Command::Console { command } => commands::console(ctx, sel, command).await,
        Command::Template { cmd } => match cmd {
            TemplateCmd::Get => commands::template_get(ctx, sel).await,
            TemplateCmd::Apply { json } => commands::template_apply(ctx, sel, json).await,
        },
        Command::Group { cmd } => match cmd {
            GroupCmd::List => commands::group_list(ctx),
        },
        Command::Watch { interval } => commands::watch(ctx, sel, *interval).await,
        // Handled before dispatch.
        Command::Schema | Command::Completions { .. } => unreachable!(),
    }
}

fn emit(output: Output) -> ExitCode {
    match output {
        Output::One(s) => {
            let _ = writeln!(std::io::stdout(), "{s}");
            ExitCode::SUCCESS
        }
        Output::Many {
            rendered,
            exit_code,
        } => {
            let _ = writeln!(std::io::stdout(), "{rendered}");
            if exit_code == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(exit_code as u8)
            }
        }
    }
}

/// Help and version print normally and exit 0; every other clap failure becomes a
/// structured `usage` error envelope.
fn handle_clap_error(e: clap::Error) -> ExitCode {
    match e.kind() {
        ClapErrorKind::DisplayHelp
        | ClapErrorKind::DisplayVersion
        | ClapErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            let _ = e.print();
            ExitCode::SUCCESS
        }
        _ => {
            let err = Error::Usage {
                message: clean_clap_message(&e),
            };
            emit_error(&err);
            ExitCode::from(err.exit_code() as u8)
        }
    }
}

/// Reduce a clap error to its core first line, dropping the multi-line
/// `Usage:` / `For more information, try '--help'.` footer and the `error:`
/// prefix, so it reads cleanly inside the structured envelope.
fn clean_clap_message(e: &clap::Error) -> String {
    let full = e.to_string();
    let first = full
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    first.strip_prefix("error: ").unwrap_or(first).to_string()
}

/// Write the clispec error envelope as the last line of stderr.
fn emit_error(err: &Error) {
    let mut error = serde_json::Map::new();
    error.insert("kind".into(), json!(err.kind()));
    error.insert("message".into(), json!(err.to_string()));
    error.insert("exit_code".into(), json!(err.exit_code()));
    if let Some(hint) = err.hint() {
        error.insert("hint".into(), json!(hint));
    }
    eprintln!("{}", json!({ "error": error }));
}
