mod campaign;
mod log_watcher;
mod overlay;
mod persistence;
mod self_update_cmd;

use std::path::PathBuf;
use std::sync::mpsc::channel;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::campaign::load_campaign_guide;
use crate::log_watcher::{LogWatcher, resolve_log_path};
use crate::overlay::{OverlayApp, viewport_options};

/// Default campaign data embedded at build time so the shipped binary does
/// not need to ship a JSON sidecar. User-specific tweaks live in the
/// optional override file under the XDG data dir (see [`default_override_path`]).
const DEFAULT_CAMPAIGN_JSON: &str = include_str!("../data/poe2_campaign.json");

#[derive(Debug, Parser)]
#[command(
    name = "rsexile",
    version,
    about = "Path of Exile 2 in-game campaign-guide overlay"
)]
struct Cli {
    /// Path to PoE2 Client.txt (auto-detected if omitted).
    #[arg(long, value_name = "PATH", global = true)]
    log: Option<PathBuf>,

    /// Path to a campaign data override JSON. Defaults to
    /// `$XDG_DATA_HOME/rsexile/poe2_campaign.local.json`.
    #[arg(long = "override-data", value_name = "PATH", global = true)]
    override_data: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Update rsexile to the latest GitHub release.
    SelfUpdate {
        /// Only report what's available; don't install.
        #[arg(long)]
        check: bool,
        /// Install a specific version tag (e.g. `v0.2.0`) instead of the latest.
        #[arg(long, value_name = "VERSION")]
        version: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::SelfUpdate { check, version }) => self_update_cmd::run(check, version),
        None => run_overlay(cli.log, cli.override_data),
    }
}

fn run_overlay(log_override: Option<PathBuf>, data_override: Option<PathBuf>) -> Result<()> {
    let override_path = data_override
        .or_else(default_override_path)
        .filter(|p| p.exists());

    let guide = load_campaign_guide(DEFAULT_CAMPAIGN_JSON, override_path.as_deref())
        .context("loading campaign data")?;

    let (events_tx, events_rx) = channel();

    // Best-effort: if Client.txt can't be found yet, run the overlay in
    // idle mode instead of crashing. The user can pass --log later, or
    // start the game and the overlay will pick up state on next launch.
    let _watcher = match resolve_log_path(log_override) {
        Ok(log_path) => {
            eprintln!("rsexile: watching {}", log_path.display());
            Some(LogWatcher::start(log_path, events_tx).context("starting log watcher")?)
        }
        Err(e) => {
            eprintln!("rsexile: {e}");
            eprintln!("rsexile: running in idle mode (no log to tail).");
            None
        }
    };

    let position = persistence::initial_position();
    let locked = persistence::load_lock_state();
    let collapsed = persistence::load_collapse_state();
    let options = viewport_options(Some(position));
    eframe::run_native(
        "rsexile",
        options,
        Box::new(move |cc| {
            Ok(Box::new(OverlayApp::new(
                cc, guide, events_rx, locked, collapsed, position,
            )))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe failed: {e}"))?;

    Ok(())
}

fn default_override_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("rsexile").join("poe2_campaign.local.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_override_path_lives_under_rsexile_data_dir() {
        // Skip on systems where dirs::data_dir() is unavailable (rare).
        let Some(path) = default_override_path() else {
            return;
        };
        let s = path.to_string_lossy();
        assert!(s.contains("rsexile"), "expected rsexile in {s}");
        assert!(
            s.ends_with("poe2_campaign.local.json"),
            "expected poe2_campaign.local.json suffix, got {s}"
        );
    }

    #[test]
    fn cli_parses_self_update_subcommand() {
        let cli = Cli::try_parse_from(["rsexile", "self-update"]).expect("parse");
        match cli.command {
            Some(Command::SelfUpdate { check, version }) => {
                assert!(!check);
                assert_eq!(version, None);
            }
            _ => panic!("expected SelfUpdate subcommand"),
        }
    }

    #[test]
    fn cli_parses_self_update_check_flag() {
        let cli = Cli::try_parse_from(["rsexile", "self-update", "--check"]).expect("parse");
        match cli.command {
            Some(Command::SelfUpdate { check, version: _ }) => assert!(check),
            _ => panic!("expected SelfUpdate subcommand"),
        }
    }

    #[test]
    fn cli_parses_self_update_pinned_version() {
        let cli =
            Cli::try_parse_from(["rsexile", "self-update", "--version", "v0.2.0"]).expect("parse");
        match cli.command {
            Some(Command::SelfUpdate { version, .. }) => {
                assert_eq!(version.as_deref(), Some("v0.2.0"));
            }
            _ => panic!("expected SelfUpdate subcommand"),
        }
    }

    #[test]
    fn cli_no_subcommand_means_run_overlay() {
        let cli = Cli::try_parse_from(["rsexile"]).expect("parse");
        assert!(cli.command.is_none());
    }

    #[test]
    fn cli_accepts_global_log_override() {
        let cli = Cli::try_parse_from(["rsexile", "--log", "/tmp/Client.txt"]).expect("parse");
        assert_eq!(cli.log, Some(PathBuf::from("/tmp/Client.txt")));
    }
}
