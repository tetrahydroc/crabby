//! `crabby mods` - read/write `mod_config.toml` and list discovered mods.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crabby_config::{ModConfig, ModEntry, Profile, discover_mods_for_config};
use crabby_error::{CrabbyError, Result};
use crabby_install::{detect_game_dir, validate_game_dir};
use tracing::info;

use crate::args::{ModsAction, ModsArgs, ProfileAction};

/// Execute the `mods` subcommand.
pub fn run(args: &ModsArgs) -> Result<()> {
    let game_dir = resolve_game_dir(args.game_dir.as_deref())?;
    match &args.action {
        ModsAction::List => list(&game_dir),
        ModsAction::Enable { id } => enable(&game_dir, id),
        ModsAction::Disable { id } => disable(&game_dir, id),
        ModsAction::Profile { action } => match action {
            ProfileAction::List => profile_list(&game_dir),
            ProfileAction::Use { name } => profile_use(&game_dir, name),
            ProfileAction::Create { name } => profile_create(&game_dir, name),
        },
    }
}

fn list(game_dir: &Path) -> Result<()> {
    let cfg = ModConfig::load_or_default(game_dir)?;
    let discovered = discover_mods_for_config(game_dir, &cfg)?;
    let profile_mods = cfg
        .active_profile()
        .map(|p| &p.mods)
        .map_or_else(Default::default, std::clone::Clone::clone);
    let enabled_count = profile_mods.values().filter(|e| e.enabled).count();

    println!(
        "Active profile: {} ({} enabled / {} in profile, {} discovered)",
        cfg.active_profile,
        enabled_count,
        profile_mods.len(),
        discovered.len(),
    );
    if discovered.is_empty() && profile_mods.is_empty() {
        println!("No mod archives found in <game-dir>/Mods/.");
        return Ok(());
    }
    println!();
    for d in &discovered {
        let entry = profile_mods.get(&d.manifest.id);
        let marker = match entry {
            Some(e) if e.enabled => "[enabled] ",
            Some(_) => "[disabled]",
            None => "[unlisted]",
        };
        let drift = entry
            .filter(|e| e.version != d.manifest.version)
            .map(|e| format!("  (was v{} when added)", e.version))
            .unwrap_or_default();
        println!(
            "  {marker}  {:<28}  v{:<10}  {}{drift}",
            d.manifest.id,
            d.manifest.version,
            d.archive_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("?"),
        );
    }

    // Mods listed in the profile but with no archive on disk; usually
    // means the .vmz was moved/deleted without disabling it first.
    let discovered_ids: HashSet<&str> = discovered.iter().map(|d| d.manifest.id.as_str()).collect();
    let orphaned: Vec<(&String, &ModEntry)> = profile_mods
        .iter()
        .filter(|(id, _)| !discovered_ids.contains(id.as_str()))
        .collect();
    if !orphaned.is_empty() {
        println!();
        println!("In profile but no archive found in Mods/ (skipped at runtime):");
        for (id, entry) in orphaned {
            let state = if entry.enabled { "enabled" } else { "disabled" };
            println!("  {id} (v{}, {state})", entry.version);
        }
    }
    Ok(())
}

fn enable(game_dir: &Path, id: &str) -> Result<()> {
    let cfg_for_discovery = ModConfig::load_or_default(game_dir)?;
    let discovered = discover_mods_for_config(game_dir, &cfg_for_discovery)?;
    let target = discovered
        .iter()
        .find(|d| d.manifest.id == id)
        .ok_or_else(|| CrabbyError::Config {
            context: format!("no archive in <game-dir>/Mods/ has id={id:?}"),
            source: "run `crabby mods list` to see available ids".into(),
        })?;

    let mut cfg = ModConfig::load_or_default(game_dir)?;
    let profile_name = cfg.active_profile.clone();
    let profile = cfg.active_profile_mut();

    match profile.mods.get_mut(id) {
        Some(entry) if entry.enabled && entry.version == target.manifest.version => {
            println!("{id} is already enabled (v{}).", entry.version);
            return Ok(());
        }
        Some(entry) => {
            let was_enabled = entry.enabled;
            let prev_version = entry.version.clone();
            entry.enabled = true;
            target.manifest.version.clone_into(&mut entry.version);
            let action = if was_enabled {
                format!(
                    "re-pinned (v{prev_version} -> v{})",
                    target.manifest.version
                )
            } else if prev_version == target.manifest.version {
                "re-enabled".to_owned()
            } else {
                format!(
                    "re-enabled (v{prev_version} -> v{})",
                    target.manifest.version
                )
            };
            cfg.save(game_dir)?;
            println!("{id}: {action} in profile {profile_name:?}.");
            info!(mod_id = id, action = action, "mod enabled");
            return Ok(());
        }
        None => {}
    }

    profile.mods.insert(
        id.to_owned(),
        ModEntry {
            enabled: true,
            version: target.manifest.version.clone(),
            priority_override: None,
        },
    );
    cfg.save(game_dir)?;
    println!(
        "Enabled {id} (v{}) in profile {profile_name:?}.",
        target.manifest.version,
    );
    info!(
        mod_id = id,
        version = target.manifest.version,
        "mod added + enabled"
    );
    Ok(())
}

fn disable(game_dir: &Path, id: &str) -> Result<()> {
    let mut cfg = ModConfig::load_or_default(game_dir)?;
    let profile_name = cfg.active_profile.clone();
    let profile = cfg.active_profile_mut();
    match profile.mods.get_mut(id) {
        None => {
            println!("{id} is not in profile {profile_name:?}; nothing to do.");
            Ok(())
        }
        Some(entry) if !entry.enabled => {
            println!("{id} is already disabled in profile {profile_name:?}.");
            Ok(())
        }
        Some(entry) => {
            entry.enabled = false;
            cfg.save(game_dir)?;
            println!("Disabled {id} in profile {profile_name:?}.");
            info!(mod_id = id, profile = profile_name, "mod disabled");
            Ok(())
        }
    }
}

fn profile_list(game_dir: &Path) -> Result<()> {
    let cfg = ModConfig::load_or_default(game_dir)?;
    println!("Profiles ({} total):", cfg.profiles.len());
    for (name, p) in &cfg.profiles {
        let marker = if name == &cfg.active_profile {
            "*"
        } else {
            " "
        };
        let enabled = p.mods.values().filter(|e| e.enabled).count();
        println!(
            "  {marker} {name:<20}  ({enabled} enabled / {} in profile)",
            p.mods.len(),
        );
    }
    Ok(())
}

fn profile_use(game_dir: &Path, name: &str) -> Result<()> {
    let mut cfg = ModConfig::load_or_default(game_dir)?;
    if !cfg.profiles.contains_key(name) {
        return Err(CrabbyError::Config {
            context: format!("no profile named {name:?}"),
            source: "create it first with `crabby mods profile create <name>`".into(),
        });
    }
    name.clone_into(&mut cfg.active_profile);
    cfg.save(game_dir)?;
    println!("Active profile is now {name:?}.");
    info!(profile = name, "active profile switched");
    Ok(())
}

fn profile_create(game_dir: &Path, name: &str) -> Result<()> {
    let mut cfg = ModConfig::load_or_default(game_dir)?;
    if cfg.profiles.contains_key(name) {
        println!("Profile {name:?} already exists.");
        return Ok(());
    }
    cfg.profiles.insert(name.to_owned(), Profile::default());
    cfg.save(game_dir)?;
    println!("Created profile {name:?}. Use `crabby mods profile use {name}` to activate it.");
    info!(profile = name, "profile created");
    Ok(())
}

fn resolve_game_dir(explicit: Option<&std::path::Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        validate_game_dir(p)?;
        return Ok(p.to_path_buf());
    }
    detect_game_dir()
}
