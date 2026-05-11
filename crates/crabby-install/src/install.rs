//! Install orchestrator.
//!
//! Owns the install pipeline:
//!
//! 1. Validate the game dir.
//! 2. Classify current `RTV.pck` (vanilla / our prior bake / unknown).
//! 3. Restore-from-backup if unknown.
//! 4. Ensure a vanilla backup exists.
//! 5. Bake the modified PCK (`bake_pck`) reading from the backup,
//!    writing to the live `RTV.pck`. Lib (the modding API + boot
//!    orchestrator) ships INSIDE the baked PCK as `res://Lib.gd`.
//! 6. Write `override.cfg` autoload entry pointing at the in-PCK Lib.
//! 7. Remove any orphaned legacy on-disk shim left over from older builds.
//! 8. Update the manifest with new vanilla + baked hashes.

use std::fs;
use std::path::Path;

use crabby_bake::{BakeKey, BakePckInputs, BakePckOutputs, bake_pck};
use crabby_error::{CrabbyError, Result};
use tracing::{debug, info, warn};

use crate::artifacts::{LEGACY_SHIM_FILE_NAME, OVERRIDE_CFG_BACKUP_NAME, OVERRIDE_CFG_NAME};
use crate::game_dir::validate_game_dir;
use crate::manifest::InstallManifest;
use crate::override_cfg;
use crate::pck_backup::{
    PckState, backup_path, classify_pck, ensure_backup, hash_file, pck_path, restore_from_backup,
};

/// What [`install`] decided to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallAction {
    /// Manifest matched the current bake key and all placed files are on
    /// disk. Nothing was written.
    AlreadyCurrent,
    /// No prior manifest found. Baked + placed from scratch.
    FreshInstall,
    /// Manifest existed but its bake key didn't match current inputs
    /// (crabby version bumped, game updated, or `force=true`). Rebaked.
    RebakedStale,
    /// Manifest's bake key was current, but a placed file was missing on
    /// disk (user deletion, antivirus, etc.). Placement was redone
    /// without rebaking.
    RepairedPlacement,
}

/// Inputs to [`install`].
#[derive(Debug)]
pub struct InstallOptions<'a> {
    /// Game directory. Must be a valid RTV install (see
    /// [`validate_game_dir`]).
    pub game_dir: &'a Path,
    /// Crabby version to record in the manifest and embed in the pack
    /// canary. Typically `env!("CARGO_PKG_VERSION")` of the driving binary.
    pub crabby_version: &'a str,
    /// Force a rebake + full re-placement even if the manifest is current.
    pub force: bool,
}

/// Report returned by [`install`].
#[derive(Debug, Clone)]
pub struct InstallReport {
    /// Which branch of the decision tree fired.
    pub action: InstallAction,
    /// Manifest as it stands after the install finished (either unchanged
    /// from disk when `AlreadyCurrent`, or freshly written otherwise).
    pub manifest: InstallManifest,
    /// PCK bake outputs. `None` when no rebake happened (`AlreadyCurrent`
    /// and `RepairedPlacement` reuse the prior `RTV.pck`).
    pub bake: Option<BakePckOutputs>,
}

/// Install (or repair) crabby in `opts.game_dir`.
///
/// The decision tree:
///
/// 1. Validate the game dir.
/// 2. Load existing manifest (if any).
/// 3. Classify the current `RTV.pck` against manifest hashes.
/// 4. If the PCK is `Unknown` (not vanilla, not crabby's), restore from
///    backup first, then continue. If no backup exists this is a hard
///    error: an unknown PCK cannot responsibly be overwritten.
/// 5. Ensure a vanilla backup exists (capture-on-first-install,
///    refresh on Steam-update drift).
/// 6. If the PCK is already the current bake AND the manifest is
///    consistent AND `!force`, no-op (`AlreadyCurrent`).
/// 7. Otherwise: bake from the vanilla backup, replacing `RTV.pck`.
/// 8. Place the shim, write `override.cfg`, save the manifest with
///    fresh hashes.
pub fn install(opts: &InstallOptions<'_>) -> Result<InstallReport> {
    validate_game_dir(opts.game_dir)?;

    let existing = InstallManifest::load(opts.game_dir)?;
    let recorded_vanilla = existing
        .as_ref()
        .and_then(|m| m.vanilla_pck_hash.as_deref());
    let recorded_baked = existing
        .as_ref()
        .and_then(|m| m.last_baked_pck_hash.as_deref());

    let state = classify_pck(opts.game_dir, recorded_vanilla, recorded_baked)?;
    debug!(?state, "current pck state");

    // Unknown means the current RTV.pck doesn't match either the
    // recorded vanilla hash or the last-baked hash. Three cases:
    //
    // 1. No manifest at all (true first install): no recorded hashes
    //    to compare against, so "unknown" is expected. Trust the
    //    current bytes as vanilla and back them up.
    // 2. Manifest exists, has a backup: restore vanilla over the
    //    unknown PCK (Steam-foreign, corruption, etc.) then proceed.
    // 3. Manifest exists, no backup: refuse. An unrecognized PCK
    //    cannot be safely overwritten without a rollback path.
    if matches!(state, PckState::Unknown { .. }) {
        if existing.is_some() {
            if backup_path(opts.game_dir).is_file() {
                warn!("current RTV.pck is unknown, restoring from vanilla backup before bake");
                restore_from_backup(opts.game_dir)?;
            } else {
                let hash = match &state {
                    PckState::Unknown { hash } => hash.as_str(),
                    _ => unreachable!(),
                };
                return Err(CrabbyError::Bake {
                    context: format!(
                        "RTV.pck has an unrecognized hash ({hash}) and no vanilla backup \
                         exists. Refusing to overwrite. Verify the game files in Steam, \
                         then re-run install."
                    ),
                    source: "no vanilla backup, refusing to clobber unknown pck".into(),
                });
            }
        }
        // No manifest: treat as first install, current bytes are
        // vanilla by definition. ensure_backup below captures them.
    }

    // Ensure backup. If RTV.pck was OursCurrent, restore vanilla into
    // it first so ensure_backup sees vanilla bytes (otherwise it would
    // back up the baked output).
    if matches!(state, PckState::OursCurrent { .. }) {
        if backup_path(opts.game_dir).is_file() {
            // Trust the backup, since ensure_backup is then a no-op.
        } else {
            return Err(CrabbyError::Bake {
                context:
                    "manifest says RTV.pck is our prior bake, but the vanilla backup is missing"
                        .into(),
                source: "cannot reconstruct vanilla without a backup".into(),
            });
        }
    } else {
        // Vanilla or freshly-restored vanilla, safe to back up.
        let _ = ensure_backup(opts.game_dir)?;
    }

    let backup = backup_path(opts.game_dir);
    let vanilla_hash = hash_file(&backup)?;

    // Pre-bake analyzer pass (no schema yet, since schema comes from
    // the bake itself). Used to compute the hook kinds map for the
    // bake's wrapper-skip + per-kind dispatch-site elision, AND to
    // fold a stable digest of the enabled-mods set into the bake key
    // so the launcher can detect mod toggles since the last bake.
    // Enabled-only: disabled mods don't ship their hooks at runtime,
    // so they shouldn't keep wrappers alive. Failures are non-fatal,
    // falling back to wrapping every method with all sites.
    let pre_bake_intents = crabby_mod_analyzer::analyze_enabled_mods(opts.game_dir)
        .inspect_err(|e| tracing::warn!(error = %e, "analyzer: pre-bake failed; wrappers will not be skipped"))
        .unwrap_or_default();

    // Hard-conflict gate: overlay collisions (two enabled mods both
    // replace_file or add_file the same target) make the bake's
    // overlay-resolve step nondeterministic - the last-write-wins
    // ordering across mods isn't a contract authors can rely on.
    // Refuse to bake until the user disables one of the conflicting
    // mods. The conflict pill in the launcher already tells them
    // which two mods are fighting; this is the enforcement layer
    // that makes the warning load-bearing.
    if let Err(e) = guard_overlay_conflicts(&pre_bake_intents) {
        return Err(e);
    }

    let hooked_kinds: std::collections::HashMap<String, crabby_rewriter::HookFlags> =
        crabby_mod_analyzer::collect_hooked_method_kinds(&pre_bake_intents)
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    crabby_rewriter::HookFlags {
                        pre: v.pre,
                        post: v.post,
                        callback: v.callback,
                        replace: v.replace,
                    },
                )
            })
            .collect();
    // Fold both the hook footprint AND the set of enabled mod IDs into
    // the digest. The IDs are what catches profile swaps where the new
    // profile enables a different set of mods that happens to declare
    // the same hook bases. Without them, the digest would match the
    // prior bake and install would short-circuit to AlreadyCurrent.
    // Same goes for no-hook mods (pure registry / UI) toggling enabled.
    let enabled_mod_ids: Vec<String> = pre_bake_intents.iter().map(|i| i.mod_id.clone()).collect();
    let hooks_and_ids_digest = crabby_bake::mods_digest_from_kinds(
        hooked_kinds
            .iter()
            .map(|(k, f)| (k.as_str(), [f.pre, f.post, f.callback, f.replace])),
        enabled_mod_ids.iter().map(String::as_str),
    );

    // Resolve overlay edits early so the AlreadyCurrent comparison sees
    // the same digest the bake would compute. Mismatch here means an
    // overlay file content changed even when the mod toggle set didn't.
    let (overlay_replacements, overlay_additions) =
        resolve_overlay_edits(opts.game_dir, &pre_bake_intents);
    let mods_digest = if overlay_replacements.is_empty() && overlay_additions.is_empty() {
        hooks_and_ids_digest
    } else {
        crabby_bake::overlay_extended_digest(
            &hooks_and_ids_digest,
            &overlay_replacements,
            &overlay_additions,
        )
    };

    // Compute the bake key against the backup (vanilla source of
    // truth) folded with the mods digest. Mismatch with the recorded
    // manifest key = bake-out-of-date.
    let current_key = BakeKey::from_pck_with_mods(opts.crabby_version, &backup, &mods_digest)?;

    // Decide: skip bake if everything is current, no force.
    //
    // No shim-drift check here: Lib (formerly the on-disk shim) now lives
    // INSIDE the baked PCK. Any change to its source bumps the bake key
    // (the PCK additions are mixed into the key alongside the vanilla
    // bytes), which already invalidates `AlreadyCurrent` via the
    // `bake_key == current_key` clause above.
    let already_current = !opts.force
        && matches!(state, PckState::OursCurrent { .. })
        && existing
            .as_ref()
            .is_some_and(|m| m.bake_key == current_key && all_placed_files_exist(opts.game_dir, m));

    if already_current {
        let manifest = existing.expect("set above");
        info!("crabby install already current ({})", manifest.bake_key);
        return Ok(InstallReport {
            action: InstallAction::AlreadyCurrent,
            manifest,
            bake: None,
        });
    }

    // Bake from vanilla backup → live RTV.pck.
    let live_pck = pck_path(opts.game_dir);
    info!(
        from = %backup.display(),
        to = %live_pck.display(),
        "running pck bake",
    );
    // Net-new entries the bake injects into the output PCK. Currently
    // just `Lib.gd`, the modding-API autoload that ships inside vanilla.
    let pck_additions: Vec<(String, Vec<u8>)> = vec![(
        crate::artifacts::LIB_PCK_PATH.to_string(),
        crate::artifacts::LIB_SOURCE.as_bytes().to_vec(),
    )];
    // Overlay edits were resolved earlier (above the AlreadyCurrent
    // check) so the digest comparison sees them; reuse those slices
    // here without re-reading every mod archive.
    let bake = bake_pck(&BakePckInputs {
        vanilla_pck: &backup,
        out_pck: &live_pck,
        crabby_version: opts.crabby_version,
        hooked_method_kinds: Some(&hooked_kinds),
        enabled_mod_ids: &enabled_mod_ids,
        pck_additions: &pck_additions,
        overlay_replacements: &overlay_replacements,
        overlay_additions: &overlay_additions,
    })?;
    let baked_hash = hash_file(&live_pck)?;

    let fresh = existing.is_none();
    let manifest = run_placement(
        opts,
        &bake.bake_key,
        existing.as_ref(),
        Some(vanilla_hash),
        Some(baked_hash),
    )?;

    // Refresh mod_index.cfg with the analyzer's overlay-source view
    // so the runtime mod-cache rebuild knows which entries to strip
    // from the runtime mount. Has to happen here (right after bake)
    // so toggle-time index refreshes can't overwrite our overlay
    // metadata with stale no-overlay-info data. Best-effort: a
    // failure here doesn't roll back the bake; the runtime falls
    // back to a per-id scan if the index is missing or stale.
    let cfg_for_index = crabby_config::ModConfig::load_or_default(opts.game_dir)?;
    let discovered_for_index =
        crabby_config::discover_mods_for_config(opts.game_dir, &cfg_for_index)?;
    let overlay_sources_by_mod_id: std::collections::BTreeMap<String, Vec<String>> =
        pre_bake_intents
            .iter()
            .filter(|i| !i.overlay_writes.is_empty())
            .map(|i| {
                let sources: Vec<String> = i
                    .overlay_writes
                    .iter()
                    .filter_map(|w| w.source_path.clone())
                    .collect();
                (i.mod_id.clone(), sources)
            })
            .collect();
    if let Err(e) = crabby_config::mod_index::rebuild_and_save_from_discovered_with_overlays(
        opts.game_dir,
        &cfg_for_index,
        &discovered_for_index,
        &overlay_sources_by_mod_id,
    ) {
        warn!(error = %e, "install: post-bake mod_index refresh failed (runtime falls back to per-id scan)");
    }

    // Post-bake mod analysis. Best-effort, since analyzer failures
    // don't block the install. The report goes to the log; the UI's
    // planned Conflicts surface will consume the same data structure
    // later.
    //
    // The bake's vanilla schema is passed so `take_over_path` /
    // `set_script` findings get function-set-comparison grading
    // instead of baseline severity.
    let schema = crabby_mod_analyzer::VanillaSchema::new(bake.vanilla_methods.clone());
    log_mod_analysis(opts.game_dir, &schema);

    Ok(InstallReport {
        action: if fresh {
            InstallAction::FreshInstall
        } else {
            InstallAction::RebakedStale
        },
        manifest,
        bake: Some(bake),
    })
}

/// Refuse to bake when any pair of enabled mods declared overlay
/// verbs targeting the same PCK path. Last-write-wins between mods
/// isn't a contract anyone should depend on, so we surface the
/// collision as an install-time hard error pointing at both mods.
///
/// Caller resolves by disabling one of the conflicting mods in the
/// launcher; the conflict chip in the Mods tab already tells them
/// which mods are fighting and on what target.
fn guard_overlay_conflicts(intents: &[crabby_mod_analyzer::ModIntent]) -> Result<()> {
    use crabby_mod_analyzer::ConflictKind;

    let conflicts = crabby_mod_analyzer::detect_conflicts(intents);
    let overlay_collisions: Vec<&crabby_mod_analyzer::Conflict> = conflicts
        .iter()
        .filter(|c| {
            matches!(
                c.kind,
                ConflictKind::FileReplaceCollision { .. } | ConflictKind::AddFileCollision { .. }
            )
        })
        .collect();

    if overlay_collisions.is_empty() {
        return Ok(());
    }

    let mut summary = String::with_capacity(overlay_collisions.len() * 96);
    for c in &overlay_collisions {
        let participants: Vec<String> = c.participants.iter().map(|p| p.mod_id.clone()).collect();
        let target = match &c.kind {
            ConflictKind::FileReplaceCollision { target } => format!("replace_file `{target}`"),
            ConflictKind::AddFileCollision { target } => format!("add_file `{target}`"),
            _ => unreachable!("filter above"),
        };
        summary.push_str(&format!(
            "  - {target} claimed by [{}]\n",
            participants.join(", ")
        ));
    }
    Err(CrabbyError::Bake {
        context: format!(
            "{} hard overlay conflict(s) prevent baking. Disable one mod from each pair, then bake again.\n{summary}",
            overlay_collisions.len(),
        ),
        source: "overlay verbs target the same PCK path across multiple enabled mods".into(),
    })
}

/// Resolve every enabled mod's overlay setup-plan intents into byte
/// slices ready for the bake pipeline.
///
/// For each `replace_file` / `add_file` intent, opens the owning mod's
/// archive (or folder), reads the source-path bytes, and returns a
/// `(target_pck_path, bytes)` tuple. Returns two slices: replacements
/// (for `replace_file`) and additions (for `add_file`).
///
/// Failures are non-fatal per intent: a missing source file logs a
/// warning and the intent is dropped from the bake. The mod analyzer
/// still surfaces the intent in the conflict surface, so the user
/// sees the misconfiguration.
/// Public alias used by [`crate::bake_status::bake_status_from_intents`]
/// so the launcher's "is bake current?" check sees the SAME overlay
/// inputs install would feed the bake. Without this alignment the
/// bake key install writes never matches the key bake_status reads
/// back, and the Launch button stays as "Bake & Launch" forever after
/// any successful overlay-bearing bake.
pub(crate) fn resolve_overlay_edits_for_intents(
    game_dir: &Path,
    intents: &[crabby_mod_analyzer::ModIntent],
) -> (Vec<(String, Vec<u8>)>, Vec<(String, Vec<u8>)>) {
    resolve_overlay_edits(game_dir, intents)
}

fn resolve_overlay_edits(
    game_dir: &Path,
    intents: &[crabby_mod_analyzer::ModIntent],
) -> (Vec<(String, Vec<u8>)>, Vec<(String, Vec<u8>)>) {
    use crabby_mod_analyzer::OverlayVerb;

    let mut replacements: Vec<(String, Vec<u8>)> = Vec::new();
    let mut additions: Vec<(String, Vec<u8>)> = Vec::new();
    if intents.iter().all(|i| i.overlay_writes.is_empty()) {
        return (replacements, additions);
    }

    // Build a (mod_id -> DiscoveredMod) map so per-intent reads can
    // open the right archive without re-scanning roots per intent.
    let cfg = match crabby_config::ModConfig::load_or_default(game_dir) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "overlay: ModConfig load failed; skipping overlay resolution");
            return (replacements, additions);
        }
    };
    let discovered = match crabby_config::discover_mods_for_config(game_dir, &cfg) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "overlay: mod discovery failed; skipping overlay resolution");
            return (replacements, additions);
        }
    };
    let by_id: std::collections::HashMap<&str, &crabby_manifest::DiscoveredMod> = discovered
        .iter()
        .map(|d| (d.manifest.id.as_str(), d))
        .collect();

    for intent in intents {
        if intent.overlay_writes.is_empty() {
            continue;
        }
        let Some(disc) = by_id.get(intent.mod_id.as_str()) else {
            tracing::warn!(
                mod_id = %intent.mod_id,
                "overlay: mod has setup-plan overlay verbs but isn't in discovered set; skipping"
            );
            continue;
        };
        for write in &intent.overlay_writes {
            let (Some(target), Some(source)) = (&write.target_path, &write.source_path) else {
                tracing::warn!(
                    mod_id = %intent.mod_id,
                    file = %write.filename,
                    line = write.line,
                    verb = write.verb.as_str(),
                    "overlay: skipping intent with non-literal target or source path"
                );
                continue;
            };
            // Strip `res://` from the source path for archive lookup;
            // the mod-relative path inside the archive is just the path
            // suffix without the protocol.
            let source_rel = source.strip_prefix("res://").unwrap_or(source.as_str());
            let bytes = match crabby_mod_analyzer::read_mod_file_bytes(disc, source_rel) {
                Ok(Some(b)) => b,
                Ok(None) => {
                    tracing::warn!(
                        mod_id = %intent.mod_id,
                        source = %source,
                        target = %target,
                        verb = write.verb.as_str(),
                        "overlay: source path not found in mod archive; skipping intent"
                    );
                    continue;
                }
                Err(e) => {
                    tracing::warn!(
                        mod_id = %intent.mod_id,
                        source = %source,
                        target = %target,
                        verb = write.verb.as_str(),
                        error = %e,
                        "overlay: failed to read source path from mod archive; skipping intent"
                    );
                    continue;
                }
            };
            match write.verb {
                OverlayVerb::ReplaceFile => replacements.push((target.clone(), bytes)),
                OverlayVerb::AddFile => additions.push((target.clone(), bytes)),
            }
        }
    }
    (replacements, additions)
}

/// Run the analyzer over enabled mods and emit a one-line summary per
/// mod plus an aggregate. Pure logging; never bails.
fn log_mod_analysis(game_dir: &Path, schema: &crabby_mod_analyzer::VanillaSchema) {
    let intents =
        match crabby_mod_analyzer::analyze_active_profile_with_schema(game_dir, Some(schema)) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "analyzer: profile analysis failed; skipping report");
                return;
            }
        };
    if intents.is_empty() {
        info!("analyzer: no enabled mods to analyze");
        return;
    }
    info!(
        target = "crabby_install::analyze",
        "=== mod analysis ({} enabled) ===",
        intents.len()
    );
    let mut total_hooks = 0usize;
    let mut total_static = 0usize;
    let mut total_reg = 0usize;
    let mut total_hard = 0usize;
    let mut total_warn = 0usize;
    let mut total_info = 0usize;
    for i in &intents {
        info!(
            target = "crabby_install::analyze",
            "  {}",
            crabby_mod_analyzer::one_line_summary(i)
        );
        total_hooks += i.hooks.len();
        total_static += i
            .hooks
            .iter()
            .filter(|h| h.resolvability == crabby_mod_analyzer::Resolvability::Static)
            .count();
        total_reg += i.registry_writes.len();
        for c in &i.classic_patterns {
            match c.severity {
                crabby_mod_analyzer::Severity::Hard => total_hard += 1,
                crabby_mod_analyzer::Severity::Warn => total_warn += 1,
                crabby_mod_analyzer::Severity::Info => total_info += 1,
            }
        }
    }
    info!(
        target = "crabby_install::analyze",
        "totals: hooks={total_hooks} (static {total_static}) reg={total_reg} classic H/W/I={total_hard}/{total_warn}/{total_info}",
    );
    if total_hard > 0 {
        info!(
            target = "crabby_install::analyze",
            "{total_hard} hard-severity classic pattern(s) detected; those mods likely won't work under crabby until they're updated"
        );
    }
}

fn all_placed_files_exist(game_dir: &Path, manifest: &InstallManifest) -> bool {
    manifest
        .placed_files
        .iter()
        .all(|rel| game_dir.join(rel).is_file())
}

/// Lay down (or relay) `override.cfg` and manifest, plus clean up any
/// legacy on-disk shim left over from older builds. The PCK itself was
/// already written in place by `bake_pck`; Lib (the modding API + boot
/// orchestrator) ships INSIDE that PCK at `res://Lib.gd`. Returns the
/// manifest as stored.
fn run_placement(
    opts: &InstallOptions<'_>,
    bake_key: &BakeKey,
    old_manifest: Option<&InstallManifest>,
    vanilla_pck_hash: Option<String>,
    last_baked_pck_hash: Option<String>,
) -> Result<InstallManifest> {
    if let Some(prev) = old_manifest {
        for rel in &prev.placed_files {
            // Keep the manifest file itself, since `save` will overwrite it.
            if rel.ends_with("install.json") {
                continue;
            }
            let abs = opts.game_dir.join(rel);
            if abs.is_file() {
                let _ = fs::remove_file(&abs);
            }
        }
    }

    let mut manifest = InstallManifest::fresh(bake_key.clone());
    manifest.vanilla_pck_hash = vanilla_pck_hash;
    manifest.last_baked_pck_hash = last_baked_pck_hash;

    // Remove any orphaned legacy on-disk shim from older crabby builds.
    // Older versions wrote `crabby_shim.gd` next to RTV.exe and listed
    // it under `[autoload_prepend]`. Boot orchestration is now baked
    // into Lib. The override.cfg rewrite below already drops the shim's
    // autoload entry, so the file would be inert, but leaving stale .gd
    // files in the game dir is confusing during uninstall and for users
    // who poke around. Best-effort: any I/O failure here is logged and
    // ignored.
    let legacy_shim = opts.game_dir.join(LEGACY_SHIM_FILE_NAME);
    if legacy_shim.is_file()
        && let Err(e) = fs::remove_file(&legacy_shim)
    {
        warn!(path = %legacy_shim.display(), error = %e, "failed to remove orphaned legacy shim, leaving it in place");
    }

    manifest.override_cfg_backup = write_override_cfg(opts, old_manifest)?;

    // Record the manifest path itself so uninstall removes it too.
    manifest.placed_files.push(".crabby/install.json".into());
    manifest.save(opts.game_dir)?;

    Ok(manifest)
}

/// Write `override.cfg`, preserving non-autoload sections from any prior
/// user-authored file. Returns the backup path (if one was taken),
/// relative to `game_dir`, for the manifest to record.
fn write_override_cfg(
    opts: &InstallOptions<'_>,
    old_manifest: Option<&InstallManifest>,
) -> Result<Option<String>> {
    let override_path = opts.game_dir.join(OVERRIDE_CFG_NAME);
    let existing = fs::read_to_string(&override_path).ok();

    // Ownership is tracked via the manifest, not an in-file marker. If
    // there's a prior manifest the override.cfg is crabby-owned and is
    // just overwritten; any `override_cfg_backup` it recorded is still
    // the backup of the user's pre-install file.
    //
    // If there's no manifest and override.cfg exists, this is a first
    // install atop a user-authored config, so back it up before
    // overwriting.
    let backup_rel: Option<String> = match (&existing, old_manifest) {
        (_, Some(prev)) => prev.override_cfg_backup.clone(),
        (Some(_), None) => {
            let backup_abs = opts.game_dir.join(OVERRIDE_CFG_BACKUP_NAME);
            fs::copy(&override_path, &backup_abs)
                .map_err(|s| CrabbyError::io_at(backup_abs.clone(), s))?;
            Some(OVERRIDE_CFG_BACKUP_NAME.into())
        }
        (None, None) => None,
    };

    let preserved = existing
        .as_deref()
        .map(override_cfg::extract_preserved_sections)
        .unwrap_or_default();
    let rendered = override_cfg::render(&preserved);

    override_cfg::write_atomically(&override_path, &rendered)?;

    Ok(backup_rel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabby_mod_analyzer::{ModIntent, OverlayVerb, OverlayWriteIntent, Resolvability};

    fn intent_with_replace(mod_id: &str, target: &str, source: &str) -> ModIntent {
        ModIntent {
            mod_id: mod_id.to_string(),
            overlay_writes: vec![OverlayWriteIntent {
                filename: "main.gd".to_string(),
                line: 1,
                verb: OverlayVerb::ReplaceFile,
                target_path: Some(target.to_string()),
                source_path: Some(source.to_string()),
                resolvability: Resolvability::Static,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn guard_passes_when_no_overlay_collisions() {
        let intents = vec![
            intent_with_replace("mod_a", "res://Scripts/Item.gd", "res://a/Item.gd"),
            intent_with_replace("mod_b", "res://Scripts/Tooltip.gd", "res://b/Tooltip.gd"),
        ];
        assert!(guard_overlay_conflicts(&intents).is_ok());
    }

    #[test]
    fn guard_blocks_when_two_mods_target_same_file() {
        let intents = vec![
            intent_with_replace("mod_a", "res://Scripts/Item.gd", "res://a/Item.gd"),
            intent_with_replace("mod_b", "res://Scripts/Item.gd", "res://b/Item.gd"),
        ];
        let err = guard_overlay_conflicts(&intents).expect_err("collision should refuse bake");
        let msg = err.to_string();
        assert!(msg.contains("res://Scripts/Item.gd"), "msg = {msg}");
        assert!(msg.contains("mod_a"), "msg = {msg}");
        assert!(msg.contains("mod_b"), "msg = {msg}");
    }

    #[test]
    fn guard_blocks_on_multiple_collisions() {
        let intents = vec![
            intent_with_replace("a", "res://Scripts/Item.gd", "res://a/Item.gd"),
            intent_with_replace("b", "res://Scripts/Item.gd", "res://b/Item.gd"),
            intent_with_replace("c", "res://Scripts/Tooltip.gd", "res://c/Tooltip.gd"),
            intent_with_replace("d", "res://Scripts/Tooltip.gd", "res://d/Tooltip.gd"),
        ];
        let err = guard_overlay_conflicts(&intents).expect_err("two collisions should refuse");
        let msg = err.to_string();
        assert!(msg.contains("Item.gd"), "msg = {msg}");
        assert!(msg.contains("Tooltip.gd"), "msg = {msg}");
    }
}
