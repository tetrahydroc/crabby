//! `crabby doctor` - wire argv to [`crabby_install::doctor`].

use std::path::{Path, PathBuf};

use crabby_error::Result;
use crabby_install::{InstallStatus, detect_game_dir, doctor};

use crate::args::DoctorArgs;

/// Execute the `doctor` subcommand.
pub fn run(args: &DoctorArgs) -> Result<()> {
    // Doctor should run even on a non-install dir so the report
    // surfaces what the tool thinks. Fall back to cwd; the report
    // will surface `InvalidGameDir` rather than erroring out.
    let game_dir = args.game_dir.as_deref().map_or_else(
        || {
            detect_game_dir()
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        },
        Path::to_path_buf,
    );

    let report = doctor(&game_dir, env!("CARGO_PKG_VERSION"))?;

    println!("game dir:      {}", report.game_dir.display());
    match &report.status {
        InstallStatus::InvalidGameDir { reason } => {
            println!("status:        invalid game directory");
            println!("reason:        {reason}");
        }
        InstallStatus::NotInstalled => {
            println!("status:        not installed");
            if let Some(key) = &report.current_bake_key {
                println!("current key:   {key}");
            }
        }
        InstallStatus::Current => {
            println!("status:        current");
            if let Some(m) = &report.manifest {
                println!("manifest key:  {}", m.bake_key);
                println!("installed at:  {}", m.installed_at);
                println!("placed files:  {}", m.placed_files.len());
            }
        }
        InstallStatus::Stale => {
            println!("status:        stale - reinstall would rebake");
            if let (Some(m), Some(cur)) = (&report.manifest, &report.current_bake_key) {
                println!("manifest key:  {}", m.bake_key);
                println!("current key:   {cur}");
            }
        }
        InstallStatus::Drifted { missing } => {
            println!("status:        drifted - {} file(s) missing", missing.len());
            for p in missing {
                println!("  missing:     {p}");
            }
        }
    }
    Ok(())
}
