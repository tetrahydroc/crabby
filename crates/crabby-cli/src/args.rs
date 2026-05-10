//! `clap`-derived CLI argument types.
//!
//! One type per subcommand, plus the top-level [`Cli`]. Keeps the argument
//! surface in one place so `--help` output stays coherent as subcommands
//! grow.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Top-level CLI entry point.
///
/// Running `crabby` with no subcommand defaults to [`Command::Install`]
/// against the detected game dir; first-run auto-install.
#[derive(Debug, Parser)]
#[command(
    name = "crabby",
    version,
    about = "Opinionated mod loader for Road to Vostok.",
    long_about = None,
)]
pub struct Cli {
    /// Subcommand to run. Omit for first-run auto-install.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Supported subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Install or refresh the crabby runtime in the game directory.
    Install(InstallArgs),
    /// Remove crabby from the game directory.
    Uninstall(UninstallArgs),
    /// Inspect install state without modifying anything.
    Doctor(DoctorArgs),
    /// Manage the active mod set (`mod_config.toml`).
    Mods(ModsArgs),
}

/// Arguments for the `install` subcommand.
#[derive(Debug, clap::Args, Default)]
pub struct InstallArgs {
    /// Path to the Road to Vostok game directory.
    ///
    /// Defaults to the current working directory, then the directory of
    /// the binary itself. Must contain `RTV.pck` and a known game binary.
    #[arg(long, value_name = "DIR")]
    pub game_dir: Option<PathBuf>,

    /// Rebake + fully re-place even if the manifest reports the install
    /// is already current. Troubleshooting hatch; not the common path.
    #[arg(long)]
    pub force: bool,
}

/// Arguments for the `uninstall` subcommand.
#[derive(Debug, clap::Args)]
pub struct UninstallArgs {
    /// Path to the Road to Vostok game directory. Same detection rules as
    /// [`InstallArgs::game_dir`].
    #[arg(long, value_name = "DIR")]
    pub game_dir: Option<PathBuf>,
}

/// Arguments for the `doctor` subcommand.
#[derive(Debug, clap::Args)]
pub struct DoctorArgs {
    /// Path to the Road to Vostok game directory. Same detection rules as
    /// [`InstallArgs::game_dir`].
    #[arg(long, value_name = "DIR")]
    pub game_dir: Option<PathBuf>,
}

/// Arguments for the `mods` subcommand.
#[derive(Debug, clap::Args)]
pub struct ModsArgs {
    /// Path to the Road to Vostok game directory. Same detection rules as
    /// [`InstallArgs::game_dir`].
    #[arg(long, value_name = "DIR", global = true)]
    pub game_dir: Option<PathBuf>,

    /// Mod-management subcommand.
    #[command(subcommand)]
    pub action: ModsAction,
}

/// Sub-actions under `crabby mods`.
#[derive(Debug, Subcommand)]
pub enum ModsAction {
    /// List discovered mod archives with active/inactive state in the
    /// active profile.
    List,
    /// Add a mod to the active profile by id.
    Enable {
        /// Mod id (from `[mod] id=` in the archive's `mod.txt`).
        id: String,
    },
    /// Remove a mod from the active profile by id.
    Disable {
        /// Mod id to remove from the active profile.
        id: String,
    },
    /// Profile management (list, switch, create).
    Profile {
        /// Profile sub-action.
        #[command(subcommand)]
        action: ProfileAction,
    },
}

/// Sub-actions under `crabby mods profile`.
#[derive(Debug, Subcommand)]
pub enum ProfileAction {
    /// List defined profiles, marking the active one.
    List,
    /// Switch the active profile to `name`. Profile must already exist.
    Use {
        /// Name of the profile to activate.
        name: String,
    },
    /// Create a new (empty) profile with `name`. Does not switch to it.
    Create {
        /// Name of the new profile.
        name: String,
    },
}
