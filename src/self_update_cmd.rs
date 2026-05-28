//! Implements the `rsexile self-update` subcommand.
//!
//! Pulls a release from `jyablonski/rsexile` via the GitHub Releases API,
//! picks the asset matching the running target triple, and atomically
//! replaces the running binary. Refuses to run on a dev build (binary
//! living under a `target/` directory) so `cargo run self-update` doesn't
//! accidentally clobber the workspace.

use anyhow::{Context, Result, bail};

const REPO_OWNER: &str = "jyablonski";
const REPO_NAME: &str = "rsexile";
const BIN_NAME: &str = "rsexile";

pub fn run(check_only: bool, target_version: Option<String>) -> Result<()> {
    refuse_dev_build().context("self-update preflight")?;

    let mut builder = self_update::backends::github::Update::configure();
    builder
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(BIN_NAME)
        .current_version(env!("CARGO_PKG_VERSION"))
        .show_download_progress(true)
        .no_confirm(false);

    if let Some(v) = target_version.as_deref() {
        builder.target_version_tag(v);
    }

    let updater = builder
        .build()
        .context("building self-update configuration")?;

    if check_only {
        let latest = updater
            .get_latest_release()
            .context("fetching latest release")?;
        println!("current: {}", env!("CARGO_PKG_VERSION"));
        println!("latest:  {}", latest.version);
    } else {
        let status = updater.update().context("running self-update")?;
        if status.updated() {
            println!("Updated to {}", status.version());
        } else {
            println!("Already up to date ({})", status.version());
        }
    }
    Ok(())
}

/// Returns an error if the current executable lives under a `target/`
/// directory (i.e. a cargo build artifact). Replacing the dev binary is
/// never what the user wanted.
fn refuse_dev_build() -> Result<()> {
    let exe = std::env::current_exe().context("locating current executable")?;
    let in_target = exe
        .components()
        .any(|c| c.as_os_str().to_string_lossy() == "target");
    if in_target {
        bail!(
            "self-update refused: this binary is inside a `target/` directory \
             (looks like a dev build). Install rsexile to ~/.local/bin and re-run."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn refuse_dev_build_blocks_target_paths() {
        // We can't easily fake current_exe() without a test harness, so
        // exercise the component-walk logic directly.
        let path = std::path::PathBuf::from("/home/u/Documents/rsexile/target/debug/rsexile");
        let in_target = path
            .components()
            .any(|c| c.as_os_str().to_string_lossy() == "target");
        assert!(in_target);
    }

    #[test]
    fn refuse_dev_build_allows_installed_paths() {
        let path = std::path::PathBuf::from("/home/u/.local/bin/rsexile");
        let in_target = path
            .components()
            .any(|c| c.as_os_str().to_string_lossy() == "target");
        assert!(!in_target);
    }
}
