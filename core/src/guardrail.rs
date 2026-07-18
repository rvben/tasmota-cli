//! Classify Tasmota commands so `console` cannot smuggle a factory-reset, firmware
//! flash, or config write past the confirmation guardrails.
//!
//! `console` can send any command, including several inside one `Backlog`. Every
//! (sub)command is classified before sending:
//! - [`Hazard::Destructive`] - resets, reflashes, or remaps hardware; always guarded.
//! - [`Hazard::RequiresConfirmation`] - not on the known-safe list (config writes,
//!   unknown commands); guarded because we cannot confirm it is safe.
//! - [`Hazard::Safe`] - a known read or basic-control command; no single-target prompt.

/// The hazard classification of a command string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hazard {
    /// At least one (sub)command resets/reflashes/remaps hardware. Carries a reason.
    Destructive(String),
    /// No destructive (sub)command, but at least one is not known-safe, so it must
    /// be confirmed before sending.
    RequiresConfirmation,
    /// Every (sub)command is a known read or basic-control command.
    Safe,
}

/// Command words that reset, reflash, or remap hardware. Uppercased for matching.
const DESTRUCTIVE: &[&str] = &[
    "RESET",        // factory / settings reset
    "UPGRADE",      // OTA firmware flash
    "UPLOAD",       // OTA upload trigger
    "OTAURL",       // sets the OTA source for a flash
    "MODULE",       // changes hardware module (remaps GPIO)
    "TEMPLATE",     // applies a GPIO template
    "GPIO",         // remaps a pin
    "GPIOS",        // remaps pins
    "WEBGETCONFIG", // pull-based config restore
    "RESTORE",      // config restore
];

/// Command words that only read state (with or without arguments). Everything not
/// here or a relay-control `POWER`, including config writes like `SetOption`,
/// `EnergyConfig`, `Sensor`, and unknown commands, requires confirmation.
const SAFE: &[&str] = &["STATUS", "STATE"];

/// Split a command into subcommands, expanding a leading `Backlog`/`Backlog0`.
fn subcommands(command: &str) -> Vec<String> {
    let trimmed = command.trim();
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let head = parts.next().unwrap_or("").trim();
    if head.eq_ignore_ascii_case("backlog") || head.eq_ignore_ascii_case("backlog0") {
        let rest = parts.next().unwrap_or("");
        return rest
            .split(';')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    vec![trimmed.to_string()]
}

/// The command word (first whitespace-delimited token), uppercased.
fn command_word(sub: &str) -> String {
    sub.split(char::is_whitespace)
        .next()
        .unwrap_or("")
        .to_ascii_uppercase()
}

/// `POWER`/`POWER1..8` are basic relay control (reversible, like the `on`/`off`
/// commands, which do not prompt for a single target).
fn is_relay_control(word: &str) -> bool {
    match word.strip_prefix("POWER") {
        Some("") => true,
        Some(rest) => rest.parse::<u8>().is_ok(),
        None => false,
    }
}

fn is_safe_word(word: &str) -> bool {
    SAFE.contains(&word) || is_relay_control(word)
}

/// Classify a raw command (possibly a `Backlog`). A destructive subcommand anywhere
/// makes the whole command destructive; otherwise any non-safe subcommand requires
/// confirmation.
pub fn classify(command: &str) -> Hazard {
    let mut requires_confirmation = false;
    for sub in subcommands(command) {
        let word = command_word(&sub);
        if DESTRUCTIVE.contains(&word.as_str()) {
            return Hazard::Destructive(format!("`{}` is a destructive command", sub.trim()));
        }
        if !is_safe_word(&word) {
            requires_confirmation = true;
        }
    }
    if requires_confirmation {
        Hazard::RequiresConfirmation
    } else {
        Hazard::Safe
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_reset_is_destructive() {
        assert!(matches!(classify("Reset 1"), Hazard::Destructive(_)));
    }

    #[test]
    fn upgrade_and_otaurl_are_destructive() {
        assert!(matches!(classify("Upgrade 1"), Hazard::Destructive(_)));
        assert!(matches!(
            classify("OtaUrl http://x/f.bin"),
            Hazard::Destructive(_)
        ));
    }

    #[test]
    fn destructive_hidden_in_backlog_is_caught() {
        assert!(matches!(
            classify("Backlog Power ON; Reset 1"),
            Hazard::Destructive(_)
        ));
        assert!(matches!(
            classify("backlog0 power on ; upgrade 1"),
            Hazard::Destructive(_)
        ));
    }

    #[test]
    fn read_and_relay_control_are_safe() {
        assert_eq!(classify("Status 0"), Hazard::Safe);
        assert_eq!(classify("Power TOGGLE"), Hazard::Safe);
        assert_eq!(classify("Backlog Power1 ON; Power2 OFF"), Hazard::Safe);
    }

    #[test]
    fn config_write_and_unknown_require_confirmation() {
        // Not destructive, but not known-safe: must be confirmed.
        assert_eq!(classify("SetOption65 1"), Hazard::RequiresConfirmation);
        assert_eq!(classify("Wifi 0"), Hazard::RequiresConfirmation);
        // Config-writing commands with arguments are not safe.
        assert_eq!(classify("EnergyConfig Full"), Hazard::RequiresConfirmation);
        assert_eq!(classify("Sensor50 1"), Hazard::RequiresConfirmation);
        assert_eq!(
            classify("Backlog Power ON; SetOption3 1"),
            Hazard::RequiresConfirmation
        );
    }
}
