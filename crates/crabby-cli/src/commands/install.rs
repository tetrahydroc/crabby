//! `crabby install` - wire argv to [`crabby_install::install`].

use std::path::PathBuf;

use crabby_bake::BakePckOutputs;
use crabby_error::Result;
use crabby_install::{InstallAction, InstallOptions, detect_game_dir, install, validate_game_dir};
use tracing::info;

use crate::args::InstallArgs;

/// Execute the `install` subcommand.
pub fn run(args: &InstallArgs) -> Result<()> {
    let game_dir = resolve_game_dir(args.game_dir.as_deref())?;

    let report = install(&InstallOptions {
        game_dir: &game_dir,
        crabby_version: env!("CARGO_PKG_VERSION"),
        force: args.force,
    })?;

    match report.action {
        InstallAction::AlreadyCurrent => {
            info!("crabby install is already current - nothing to do");
            println!("Already installed and current. Launch the game to verify.");
        }
        InstallAction::FreshInstall => {
            info!(
                files = report.manifest.placed_files.len(),
                "fresh crabby install complete",
            );
            println!(
                "Installed. {} file(s) placed in {}.",
                report.manifest.placed_files.len(),
                game_dir.display(),
            );
            print_bake_stats(report.bake.as_ref());
        }
        InstallAction::RebakedStale => {
            info!(
                key = %report.manifest.bake_key,
                "stale install detected - rebaked",
            );
            println!(
                "Rebaked and updated. Bake key now {}.",
                report.manifest.bake_key,
            );
            print_bake_stats(report.bake.as_ref());
        }
        InstallAction::RepairedPlacement => {
            info!("placement repaired (pack unchanged)");
            println!("Placement repaired. Bake cache was current and reused.");
        }
    }
    Ok(())
}

/// Resolve the game directory: explicit arg wins; otherwise auto-detect.
fn resolve_game_dir(explicit: Option<&std::path::Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        validate_game_dir(p)?;
        return Ok(p.to_path_buf());
    }
    detect_game_dir()
}

/// Print bake counts to stdout. No-op when `bake` is `None` (the
/// already-current branch reuses the prior `RTV.pck`).
fn print_bake_stats(bake: Option<&BakePckOutputs>) {
    let Some(b) = bake else { return };
    let s = &b.stats;
    println!(
        "Bake: {} script(s) rewritten in PCK ({} entries total), {} hook(s) instrumented.",
        b.scripts_rewritten, b.total_entries, s.total_hooks,
    );
    println!(
        "  templates: void={} non_void={} fast={} additive={}",
        s.void_hooks, s.non_void_hooks, s.fast_hooks, s.additive_hooks,
    );
    println!(
        "  additive scripts: {} ({} method name(s) drive consumer-call rewrites)",
        s.additive_scripts, s.additive_method_names,
    );
    println!("  data-intercept scripts: {}", s.data_intercept_scripts);
}
