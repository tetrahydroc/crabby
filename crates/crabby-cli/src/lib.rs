//! `crabby` CLI - invoked from the unified `crabby.exe` binary when
//! a subcommand is passed. The entry point is [`run`], which parses
//! args + dispatches into the same handlers the standalone `crabby`
//! binary used to run before the launcher absorbed it.
//!
//! When `crabby.exe` is invoked with no subcommand it launches the UI
//! (handled in `crabby-ui/src/main.rs`); `run()` is not reached in
//! that case.

#![deny(missing_docs)]

pub mod args;
pub mod commands;
mod exit;

use std::process::ExitCode;

use crabby_platform::observability::init_tracing;
use tracing::info;

use crate::args::{Cli, Command, InstallArgs};

/// Run the CLI. Parses `std::env::args` via `clap` and dispatches to
/// the matching subcommand handler. Returns the exit code the host
/// binary should propagate.
///
/// Initializes tracing on entry; callers must NOT install their own
/// subscriber first or this will panic on the duplicate registration.
#[must_use]
pub fn run() -> ExitCode {
    init_tracing();
    info!(version = env!("CARGO_PKG_VERSION"), "crabby cli starting");

    let cli = <Cli as clap::Parser>::parse();

    let result = match cli.command {
        Some(Command::Install(args)) => commands::install::run(&args),
        Some(Command::Uninstall(args)) => commands::uninstall::run(&args),
        Some(Command::Doctor(args)) => commands::doctor::run(&args),
        Some(Command::Mods(args)) => commands::mods::run(&args),
        // Reached when the launcher's main.rs forwards `crabby` (no
        // subcommand) to the CLI; the legacy default of `install` is
        // kept for that path. With the unified binary, no-args is
        // intercepted before reaching here and routed to the UI;
        // this arm covers explicit `crabby --help`-then-no-subcommand
        // and similar edge cases.
        None => commands::install::run(&InstallArgs::default()),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => exit::report(&err),
    }
}

/// Subcommand names that should route to the CLI when seen as the
/// first positional arg of `crabby.exe`. Used by the launcher's
/// dispatcher to decide between UI mode and CLI mode without having
/// to commit to a clap parse pass first (clap on a UI-mode invocation
/// would consume the args and lose them).
///
/// Keep in sync with [`Command`].
pub const CLI_SUBCOMMANDS: &[&str] = &[
    "install",
    "uninstall",
    "doctor",
    "mods",
    // clap's auto-generated help/version flags - these route to CLI
    // handling, not UI startup.
    "help",
    "--help",
    "-h",
    "--version",
    "-V",
];
