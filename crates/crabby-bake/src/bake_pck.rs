//! Direct-PCK bake: rewrites `Scripts/*.gdc` entries in vanilla `RTV.pck`
//! to `Scripts/*.gd` text source via the rewriter pipeline, writing the
//! result as a fresh PCK at `out_pck`.
//!
//! Replaces the side-pack approach in [`crate::bake::bake_pack`]: instead
//! of layering a hook pack over vanilla via `override.cfg`, this owns the
//! bytes Godot loads as the main pack. That makes `_get`-based data
//! intercepts work for resource scripts (which the side-pack approach
//! couldn't bind, see `docs/PCK_REWRITE_PLAN.md`).
//!
//! # Pipeline
//!
//! 1. Open the vanilla PCK (typically `RTV.pck.vanilla.bak`).
//! 2. Pass 1: for every `Scripts/*.gdc`, detokenize, parse,
//!    `rewrite_full_script`. Skip zero-byte entries and runtime-
//!    incompatible scripts (their vanilla bytes pass through unchanged).
//! 3. Pass 2: rewrite consumer call sites on non-additive scripts to
//!    redirect into additive hooks (same as `bake_pack`).
//! 4. Stream every PCK entry through [`PckArchive::rewrite_to`]: for
//!    each rewritten `Scripts/X.gdc`, emit `Scripts/X.gd` with the
//!    rewritten text as bytes; everything else passes through verbatim.
//!
//! Output PCK is written atomically (`<out>.tmp` + rename inside
//! [`PckArchive::rewrite_to`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crabby_detokenizer::detokenize;
use crabby_error::{CrabbyError, Result};
use crabby_parser::ParsedScript;
use crabby_parser::parse_script;
use crabby_pck::{PckArchive, PckEntry};
use crabby_rewriter::{
    ADDITIVE_TEMPLATE_SCRIPTS, TemplateKind, is_additive_script, is_data_intercept_script,
    is_runtime_incompatible, pick_template, rewrite_consumer_calls, rewrite_full_script_with_hooks,
};
use std::collections::HashSet;
use tracing::{debug, info};

use crate::bake::BakeStats;
use crate::bake_key::BakeKey;

/// Inputs to [`bake_pck`].
#[derive(Debug)]
pub struct BakePckInputs<'a> {
    /// Vanilla PCK to read from. Must remain unmodified, since
    /// production callers point this at the vanilla backup
    /// (`RTV.pck.vanilla.bak`), not the live `RTV.pck`.
    pub vanilla_pck: &'a Path,
    /// Destination path for the rewritten PCK. Typically the live
    /// `RTV.pck` itself; the writer's atomic-rename ensures the live
    /// file isn't half-written if the bake crashes.
    pub out_pck: &'a Path,
    /// Crabby version, embedded in the bake key.
    pub crabby_version: &'a str,
    /// Per-base map of which hook KINDS the active mod profile
    /// registers (`pre` / `post` / `callback` / `replace`). When
    /// `Some`:
    ///   * Bases absent from the map cause the wrapper to be skipped
    ///     entirely (AOT skip).
    ///   * Bases present cause the wrapper to be emitted, but each
    ///     per-kind dispatch line is elided when its flag is `false`
    ///     (partial-emit). `replace = false` also drops the entire
    ///     `_get_hooks` / super-skip branch.
    /// `None` keeps legacy behavior (wrap every hookable method
    /// with all four dispatch sites) for callers without analyzer
    /// data, e.g. rewriter unit tests.
    pub hooked_method_kinds:
        Option<&'a std::collections::HashMap<String, crabby_rewriter::HookFlags>>,
    /// Set of enabled-mod IDs in the active profile, folded into the
    /// bake key alongside `hooked_method_kinds`. Catches profile
    /// swaps where the new profile has the same hook footprint but a
    /// different mod set (and the no-hook-mod toggle case where the
    /// hook footprint is unchanged but a pure-registry mod was
    /// added/removed). Empty slice keeps the legacy key shape for
    /// callers without analyzer data.
    pub enabled_mod_ids: &'a [String],
    /// Net-new files to inject into the output PCK as fresh entries.
    /// Each tuple is `(res_path, bytes)`, e.g. `("res://Lib.gd",
    /// lib_source_bytes)` to ship the in-PCK modding-API autoload.
    /// Caller is responsible for ensuring `res_path` doesn't collide
    /// with any existing source-PCK entry.
    pub pck_additions: &'a [(String, Vec<u8>)],
}

/// Outputs from [`bake_pck`].
#[derive(Debug, Clone)]
pub struct BakePckOutputs {
    /// Absolute path of the emitted PCK (same as `inputs.out_pck`).
    pub pck_path: PathBuf,
    /// Number of script entries rewritten (`.gdc` → `.gd`).
    pub scripts_rewritten: usize,
    /// Total entries in the output PCK (rewritten + passthrough).
    pub total_entries: usize,
    /// Diagnostic counts for the bake.
    pub stats: BakeStats,
    /// Bake key identifying this PCK's inputs.
    pub bake_key: BakeKey,
    /// Side-output: vanilla method-name set per `Scripts/<X>.gd`.
    /// Free byproduct of pass 1 (every script is already parsed). The
    /// mod analyzer consumes this to grade `take_over_path`/`set_script`
    /// findings by comparing the swapping script's method set against
    /// vanilla's, where empty intersection = additive (Info), non-empty
    /// = likely override (Warn/Hard depending on overlap size).
    ///
    /// Filename keys are bare (`Item.gd`, not `res://Scripts/Item.gd`)
    /// so consumers can match against [`crate::script_filename`]-style
    /// names directly.
    pub vanilla_methods: BTreeMap<String, BTreeSet<String>>,
}

/// Per-script intermediate carrier between pass 1 and pass 2.
struct Pass1 {
    /// Original PCK entry path (e.g. `res://Scripts/Item.gdc`).
    src_path: String,
    /// Filename used for parser/template lookups (e.g. `Item.gd`).
    filename: String,
    /// Rewritten source from pass 1.
    source: String,
    is_additive: bool,
    parsed: ParsedScript,
}

/// Bake the full vanilla `Scripts/` corpus into a modified PCK.
pub fn bake_pck(inputs: &BakePckInputs<'_>) -> Result<BakePckOutputs> {
    // Fold the active enabled-mods' hook kinds into the bake key so the
    // launcher can detect "a mod was toggled since last bake, PCK is
    // out of date" by comparing keys. Empty when caller didn't pass
    // analyzer data (CLI / tests), preserving the legacy key shape.
    // Hook footprint + enabled-mod IDs both feed the digest. IDs catch
    // profile swaps where two profiles enable different mods that
    // happen to declare the same hook bases (and the no-hook-mod case).
    // When `hooked_method_kinds` is `None` AND `enabled_mod_ids` is
    // empty (CLI / tests without analyzer data) the digest is empty,
    // preserving the legacy three-component key shape.
    let empty_kinds: std::collections::HashMap<String, crabby_rewriter::HookFlags> =
        std::collections::HashMap::new();
    let kinds_map = inputs.hooked_method_kinds.unwrap_or(&empty_kinds);
    let mods_digest = crate::mods_digest_from_kinds(
        kinds_map
            .iter()
            .map(|(k, f)| (k.as_str(), [f.pre, f.post, f.callback, f.replace])),
        inputs.enabled_mod_ids.iter().map(String::as_str),
    );
    let bake_key =
        BakeKey::from_pck_with_mods(inputs.crabby_version, inputs.vanilla_pck, &mods_digest)?;
    let mut archive = PckArchive::open(inputs.vanilla_pck)?;

    // Filter to script entries first so pass-1 only iterates them.
    let script_entries: Vec<PckEntry> = archive
        .entries()
        .iter()
        .filter(|e| {
            std::path::Path::new(&e.path)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("gdc"))
        })
        .filter(|e| {
            let n = e.path.trim_start_matches("res://").trim_start_matches('/');
            n.starts_with("Scripts/")
        })
        .cloned()
        .collect();

    if script_entries.is_empty() {
        return Err(CrabbyError::Bake {
            context: format!(
                "no Scripts/*.gdc entries in {}; not an RTV PCK?",
                inputs.vanilla_pck.display(),
            ),
            source: "empty Scripts directory".into(),
        });
    }

    // ---- Pass 1: parse + rewrite each script.
    let mut rewritten: Vec<Pass1> = Vec::with_capacity(script_entries.len());
    let mut additive_methods: HashSet<String> = HashSet::new();
    let mut zero_byte_skipped = 0usize;
    let mut runtime_incompatible_skipped = 0usize;
    let mut stats = BakeStats::default();
    // Vanilla schema, method names per script. Built incrementally
    // alongside the rewrite loop; surfaced on `BakePckOutputs` for the
    // mod analyzer's classic-pattern severity scoring.
    let mut vanilla_methods: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for entry in &script_entries {
        let filename = script_filename(&entry.path);
        if is_runtime_incompatible(&filename) {
            runtime_incompatible_skipped += 1;
            continue;
        }
        let bytes = archive.read(entry)?;
        let source = detokenize(&bytes)?;
        if source.is_empty() {
            zero_byte_skipped += 1;
            continue;
        }

        let parsed = parse_script(&filename, &source)?;

        // Capture method names for the vanilla schema. Includes static
        // methods, since mods don't typically `take_over_path` swap
        // classes with statics, but the analyzer's overlap check
        // shouldn't treat a vanilla static as "missing" when comparing.
        let methods: BTreeSet<String> = parsed.functions.iter().map(|f| f.name.clone()).collect();
        if !methods.is_empty() {
            vanilla_methods.insert(filename.clone(), methods);
        }

        let rewritten_source =
            rewrite_full_script_with_hooks(&source, &parsed, inputs.hooked_method_kinds)?;
        let is_additive = is_additive_script(&filename);
        let is_data_intercept = is_data_intercept_script(&filename);

        // All hookable methods are "candidates" for wrapping. With
        // an analyzer-supplied kinds map, only methods whose base
        // name is in the map get wrapped (AOT skip); each emitted
        // wrapper further elides per-kind dispatch sites for kinds
        // nobody hooks.
        let candidates: Vec<_> = parsed.functions.iter().filter(|f| !f.is_static).collect();
        let prefix = crabby_rewriter::script_prefix(&filename);
        let hookable: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|f| match inputs.hooked_method_kinds {
                None => true,
                Some(map) => map.contains_key(&crabby_rewriter::hook_base(&prefix, &f.name)),
            })
            .collect();
        stats.candidate_methods += candidates.len();
        stats.wrappers_skipped_aot += candidates.len() - hookable.len();
        // Per-kind savings: for every wrapper that WAS emitted, count
        // how many dispatch sites were dropped.
        if let Some(map) = inputs.hooked_method_kinds {
            for func in &hookable {
                let base = crabby_rewriter::hook_base(&prefix, &func.name);
                let f = map
                    .get(&base)
                    .copied()
                    .unwrap_or(crabby_rewriter::HookFlags::all());
                if !f.pre {
                    stats.pre_sites_skipped += 1;
                }
                if !f.post {
                    stats.post_sites_skipped += 1;
                }
                if !f.callback {
                    stats.callback_sites_skipped += 1;
                }
                if !f.replace {
                    stats.replace_branches_skipped += 1;
                }
            }
        }
        if !hookable.is_empty() {
            stats.scripts_with_hooks += 1;
        }
        if is_additive {
            stats.additive_scripts += 1;
        }
        if is_data_intercept {
            stats.data_intercept_scripts += 1;
        }
        for func in &hookable {
            stats.total_hooks += 1;
            match pick_template(&parsed, func) {
                TemplateKind::Additive => stats.additive_hooks += 1,
                TemplateKind::Fast => stats.fast_hooks += 1,
                TemplateKind::Void => stats.void_hooks += 1,
                TemplateKind::NonVoid => stats.non_void_hooks += 1,
            }
        }
        if is_additive {
            for func in &hookable {
                additive_methods.insert(func.name.clone());
            }
        }

        rewritten.push(Pass1 {
            src_path: entry.path.clone(),
            filename,
            source: rewritten_source,
            is_additive,
            parsed,
        });
    }

    stats.additive_method_names = additive_methods.len();
    debug!(
        scripts = rewritten.len(),
        zero_byte = zero_byte_skipped,
        runtime_incompatible = runtime_incompatible_skipped,
        additive_methods = additive_methods.len(),
        "pck bake pass 1 complete",
    );

    // ---- Pass 2: redirect consumer call sites on non-additive scripts.
    let additive_refs: HashSet<&str> = additive_methods.iter().map(String::as_str).collect();
    let additive_types: HashSet<&str> = ADDITIVE_TEMPLATE_SCRIPTS
        .iter()
        .filter_map(|f| f.strip_suffix(".gd"))
        .collect();

    // Build a map from original PCK path → replacement (new_path, new_bytes).
    //
    // Each rewritten script needs TWO output entries to satisfy
    // Godot's resource loader:
    //
    //   1. The `.gd` text source at `Scripts/X.gd`, the entry to be loaded.
    //   2. The `.gd.remap` redirect at `Scripts/X.gd.remap` containing
    //      `[remap]\npath="res://Scripts/X.gd"`. Vanilla's PCK ships
    //      a `.gd.remap` pointing at `.gdc`; leaving it in place causes
    //      every `.gd` lookup to be routed to a `.gdc` that was deleted,
    //      and the engine errors with "Failed to open binary GDScript
    //      file". Vanilla's redirect is overwritten with a self-pointing
    //      one so the `.gd` text wins.
    //
    // The original `.gdc` entry is dropped; `.remap` resolution finds
    // the text first and the bytecode is never consulted.
    //
    // Map shape: src_path → Replacement{ primary: (new_path, bytes),
    //                                    extras: Vec<(path, bytes)> }
    struct Replacement {
        new_path: String,
        new_bytes: Vec<u8>,
        // Extra entries to emit on the same pass (e.g. the .gd.remap).
        extras: Vec<(String, Vec<u8>)>,
    }

    let mut replacements: HashMap<String, Replacement> = HashMap::with_capacity(rewritten.len());
    for p in rewritten {
        let final_source = if p.is_additive || additive_refs.is_empty() {
            p.source
        } else {
            rewrite_consumer_calls(&p.source, &p.parsed, &additive_refs, &additive_types)
        };
        let gd_path = swap_gdc_to_gd(&p.src_path);
        // Self-pointing remap. Format mirrors editor_export_platform.cpp
        // which writes `[remap]\n\npath="..."\n`. Godot's parser is
        // lenient on whitespace but stick to the canonical form.
        let remap_path = format!("{gd_path}.remap");
        let res_path = if gd_path.starts_with("res://") {
            gd_path.clone()
        } else {
            format!("res://{}", gd_path.trim_start_matches('/'))
        };
        let remap_body = format!("[remap]\n\npath=\"{res_path}\"\n");
        replacements.insert(
            p.src_path,
            Replacement {
                new_path: gd_path,
                new_bytes: final_source.into_bytes(),
                extras: vec![(remap_path, remap_body.into_bytes())],
            },
        );
        let _ = p.filename;
    }

    let scripts_rewritten = replacements.len();

    // Vanilla also ships a `.gd.remap` for every script (175 of them in
    // RTV 4.6.2) that points `.gd` → `.gdc`. Those remaps must be
    // OVERWRITTEN, not duplicated. The closure handles two cases:
    //
    //   - Source entry is `Scripts/X.gdc`: drop original (return Some
    //     with the `.gd` text under the new path; the original `.gdc`
    //     entry is gone).
    //   - Source entry is `Scripts/X.gd.remap`: replace bytes with the
    //     self-pointing remap, keep path the same.
    //
    // Extras (like a freshly-emitted `.gd.remap` for a script whose
    // vanilla `.gd.remap` somehow doesn't exist) are written via the
    // returned-via-extras side channel; they are accumulated, then
    // rewrite_to writes them as additional entries. Since rewrite_to
    // doesn't currently support additions, the vanilla `.gd.remap`
    // entry's pass-through hook writes the content instead.
    //
    // Build a parallel map: vanilla `.gd.remap` path → desired remap body.
    let mut remap_overrides: HashMap<String, Vec<u8>> = HashMap::new();
    for (_src, rep) in &replacements {
        for (path, body) in &rep.extras {
            remap_overrides.insert(path.clone(), body.clone());
        }
    }

    // ---- Stream every entry through rewrite_to, applying replacements
    //      and appending net-new entries from `inputs.pck_additions`
    //      (e.g. `Lib.gd`, see crate docs).
    archive.rewrite_to_with_additions(
        inputs.out_pck,
        |entry, _bytes| {
            if let Some(rep) = replacements.remove(&entry.path) {
                return Some((rep.new_path, rep.new_bytes));
            }
            if let Some(body) = remap_overrides.remove(&entry.path) {
                return Some((entry.path.clone(), body));
            }
            None
        },
        inputs.pck_additions.to_vec(),
    )?;

    // Re-open to get the final entry count for the report.
    let total_entries = PckArchive::open(inputs.out_pck)?.entries().len();

    info!(
        out = %inputs.out_pck.display(),
        scripts_rewritten,
        total_entries,
        zero_byte_skipped,
        runtime_incompatible_skipped,
        scripts_with_hooks = stats.scripts_with_hooks,
        total_hooks = stats.total_hooks,
        candidate_methods = stats.candidate_methods,
        wrappers_skipped_aot = stats.wrappers_skipped_aot,
        pre_sites_skipped = stats.pre_sites_skipped,
        post_sites_skipped = stats.post_sites_skipped,
        callback_sites_skipped = stats.callback_sites_skipped,
        replace_branches_skipped = stats.replace_branches_skipped,
        additive_scripts = stats.additive_scripts,
        additive_hooks = stats.additive_hooks,
        fast_hooks = stats.fast_hooks,
        void_hooks = stats.void_hooks,
        non_void_hooks = stats.non_void_hooks,
        data_intercept_scripts = stats.data_intercept_scripts,
        additive_method_names = stats.additive_method_names,
        "pck bake complete",
    );

    Ok(BakePckOutputs {
        pck_path: inputs.out_pck.to_path_buf(),
        scripts_rewritten,
        total_entries,
        stats,
        bake_key,
        vanilla_methods,
    })
}

fn script_filename(path: &str) -> String {
    let normalized = path.trim_start_matches("res://").trim_start_matches('/');
    let stem = std::path::Path::new(normalized)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    format!("{stem}.gd")
}

/// `res://Scripts/Foo.gdc` → `res://Scripts/Foo.gd`. Godot's resource
/// loader resolves either extension; ships `.gd` so the engine
/// compiles from text.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn swap_gdc_to_gd(path: &str) -> String {
    if let Some(stem) = path.strip_suffix(".gdc") {
        format!("{stem}.gd")
    } else if let Some(stem) = path.strip_suffix(".GDC") {
        format!("{stem}.gd")
    } else {
        path.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_gdc_to_gd_swaps_lowercase_extension() {
        assert_eq!(
            swap_gdc_to_gd("res://Scripts/Item.gdc"),
            "res://Scripts/Item.gd",
        );
    }

    #[test]
    fn swap_gdc_to_gd_swaps_uppercase_extension() {
        assert_eq!(
            swap_gdc_to_gd("res://Scripts/Item.GDC"),
            "res://Scripts/Item.gd",
        );
    }

    #[test]
    fn swap_gdc_to_gd_passes_other_paths_through() {
        assert_eq!(
            swap_gdc_to_gd("res://Items/Potato.tres"),
            "res://Items/Potato.tres",
        );
        assert_eq!(
            swap_gdc_to_gd("res://Scripts/Already.gd"),
            "res://Scripts/Already.gd",
        );
    }

    #[test]
    fn script_filename_strips_dirs_and_extension() {
        assert_eq!(script_filename("res://Scripts/Item.gdc"), "Item.gd");
        assert_eq!(script_filename("Scripts/Sub/X.gdc"), "X.gd");
    }
}
