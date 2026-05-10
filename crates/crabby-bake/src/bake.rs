//! Full-corpus bake.
//!
//! Iterates every `Scripts/*.gdc` in the supplied PCK, runs the rewriter
//! pipeline (detokenize, parse, `rewrite_full_script`), then runs the
//! consumer call-site rewriter against non-additive scripts to redirect
//! `<receiver>.<method>(...)` calls into additive-template hooked
//! variants. Emits the result as a single hook pack archive.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crabby_detokenizer::detokenize;
use crabby_error::{CrabbyError, Result};
use crabby_pack::{PackInputs, RewrittenScript, emit_pack};
use crabby_parser::ParsedScript;
use crabby_parser::parse_script;
use crabby_pck::{PckArchive, PckEntry};
use crabby_rewriter::{
    ADDITIVE_TEMPLATE_SCRIPTS, TemplateKind, is_additive_script, is_data_intercept_script,
    is_runtime_incompatible, pick_template, rewrite_consumer_calls, rewrite_full_script,
};
use tracing::{debug, info};

use crate::bake_key::BakeKey;

/// Inputs to [`bake_pack`].
#[derive(Debug)]
pub struct BakeInputs<'a> {
    /// Path to `RTV.pck`.
    pub pck_path: &'a Path,
    /// Destination path for the emitted hook pack
    /// (e.g. `<game-dir>/crabby_hook_pack.zip`).
    pub out_pack: &'a Path,
    /// Crabby version, embedded in the pack canary and contributes to
    /// the returned [`BakeKey`].
    pub crabby_version: &'a str,
}

/// Outputs from [`bake_pack`].
#[derive(Debug, Clone)]
pub struct BakeOutputs {
    /// Absolute path of the emitted pack.
    pub pack_path: PathBuf,
    /// Number of scripts packed (one entry per non-zero-byte vanilla
    /// script in `Scripts/`).
    pub script_count: usize,
    /// Number of scripts skipped because they were zero-byte in the PCK
    /// (nothing to detokenize).
    pub zero_byte_skipped: usize,
    /// Diagnostic counts for the bake, recording what got wrapped and how.
    pub stats: BakeStats,
    /// Bake key identifying this pack's inputs. Callers persist this in
    /// their manifest so they can skip the rebake on subsequent runs
    /// when the key hasn't changed.
    pub bake_key: BakeKey,
}

/// Per-bake diagnostic counts.
///
/// Populated as the rewriter walks the corpus so callers can surface what
/// crabby actually did. Cheap to compute, since the loop already inspects
/// every function for template selection.
#[derive(Debug, Clone, Default)]
pub struct BakeStats {
    /// Scripts with at least one hookable (non-static) method.
    pub scripts_with_hooks: usize,
    /// Scripts whose `filename` matched the additive list.
    pub additive_scripts: usize,
    /// Scripts whose `filename` matched the data-intercept list.
    pub data_intercept_scripts: usize,
    /// Total per-method dispatch wrappers emitted across the corpus,
    /// the headline number for "how many hooks were added".
    pub total_hooks: usize,
    /// Hooks emitted via the additive template (vanilla body retained,
    /// hooked sibling alongside).
    pub additive_hooks: usize,
    /// Hooks emitted via the fast template.
    pub fast_hooks: usize,
    /// Hooks emitted via the void template.
    pub void_hooks: usize,
    /// Hooks emitted via the non-void template.
    pub non_void_hooks: usize,
    /// Distinct additive method names contributed to the consumer-rewrite
    /// pass. Drives how many vanilla call sites get redirected.
    pub additive_method_names: usize,
    /// Methods that would have been wrapped in legacy mode but were
    /// passed through vanilla because no active mod hooks them
    /// (AOT skip). `0` when no hook-set was provided.
    pub wrappers_skipped_aot: usize,
    /// Total non-static methods seen across the corpus. The "wrapper
    /// hit rate" is `1 - wrappers_skipped_aot / candidate_methods`,
    /// a useful denominator for the bake report.
    pub candidate_methods: usize,
    /// Per-kind dispatch sites elided from emitted wrappers
    /// because no active mod hooked that kind. `pre_sites_skipped`
    /// counts wrappers that omitted their `-pre` dispatch line, etc.
    /// `replace_branches_skipped` counts wrappers that dropped the
    /// entire `_get_hooks` / replace probe block (call vanilla direct).
    pub pre_sites_skipped: usize,
    /// See [`Self::pre_sites_skipped`].
    pub post_sites_skipped: usize,
    /// See [`Self::pre_sites_skipped`].
    pub callback_sites_skipped: usize,
    /// See [`Self::pre_sites_skipped`].
    pub replace_branches_skipped: usize,
}

/// Per-script carrier for pass-1 outputs fed into pass 2.
struct Pass1 {
    filename: String,
    source: String,
    is_additive: bool,
    parsed: ParsedScript,
}

/// Bake the full vanilla `Scripts/` corpus into a single hook pack.
///
/// # Pipeline
///
/// 1. Open the PCK and enumerate every `Scripts/*.gdc` entry.
/// 2. For each entry: detokenize, parse, `rewrite_full_script`.
///    Skip silently if the entry's bytes are zero (PCK ships some
///    `.gdc` entries empty; nothing to rewrite).
/// 3. Collect the set of method names that belong to additive-template
///    scripts (save-serialized resources whose method names are
///    embedded in saves). These are the only consumer-rewrite targets.
/// 4. Run [`rewrite_consumer_calls`] on every **non-additive** script's
///    output, redirecting `<receiver>.<additive_method>(...)` calls
///    into their `_rtv_hooked_` variants.
/// 5. Emit a hook pack containing all rewritten sources.
///
/// # Errors
///
/// Sub-crate errors propagate as-is. PCK-level failures convert into
/// [`CrabbyError::Bake`] with explanatory context.
pub fn bake_pack(inputs: &BakeInputs<'_>) -> Result<BakeOutputs> {
    let bake_key = BakeKey::from_pck(inputs.crabby_version, inputs.pck_path)?;
    let mut archive = PckArchive::open(inputs.pck_path)?;

    let entries: Vec<PckEntry> = archive
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

    if entries.is_empty() {
        return Err(CrabbyError::Bake {
            context: format!(
                "no Scripts/*.gdc entries in {}; not an RTV PCK?",
                inputs.pck_path.display(),
            ),
            source: "empty Scripts directory".into(),
        });
    }

    // Pass 1: rewrite each script. Track additive-method names for the
    // consumer-rewrite pass below, plus diagnostic counts for the
    // report. Keep each script's `ParsedScript` around because pass 2
    // needs the typed-decl table to type-check call receivers.
    let mut rewritten: Vec<Pass1> = Vec::with_capacity(entries.len());
    let mut additive_methods: HashSet<String> = HashSet::new();
    let mut zero_byte_skipped = 0usize;
    let mut stats = BakeStats::default();

    let mut runtime_incompatible_skipped = 0usize;
    let mut additive_skipped = 0usize;
    for entry in &entries {
        let filename = script_filename(&entry.path);
        // Skip scripts whose source can't be re-compiled at runtime.
        // No rewritten entry is placed in the pack at all; vanilla
        // `.gdc` from RTV.pck keeps serving the original behavior via
        // VFS fallthrough. Hooks on these scripts won't fire; install
        // diagnostics surface this.
        if is_runtime_incompatible(&filename) {
            runtime_incompatible_skipped += 1;
            continue;
        }
        // Skip resource-serialized (additive) scripts entirely. Any pack
        // entry for these (even just `.gd` + `.gd.remap`) breaks
        // Godot's resource-script class binding for `.tres` references
        // pointing at the script's path: the bound script becomes a
        // generic `Resource` and downstream code that reads typed fields
        // sees stale defaults (symptom: `set_name("")` flood after
        // `Update` "ran"). Vanilla PCK bytecode serves these scripts
        // via VFS fallthrough; hooks on their methods won't fire.
        // Mirrors vostok-mod-loader's `RTV_RESOURCE_SERIALIZED_SKIP`
        // bypass at `src/hook_pack.gd:310`.
        if is_additive_script(&filename) {
            additive_skipped += 1;
            continue;
        }
        let bytes = archive.read(entry)?;
        let source = detokenize(&bytes)?;
        if source.is_empty() {
            zero_byte_skipped += 1;
            continue;
        }

        let parsed = parse_script(&filename, &source)?;
        let rewritten_source = rewrite_full_script(&source, &parsed)?;
        let is_additive = is_additive_script(&filename);
        let is_data_intercept = is_data_intercept_script(&filename);

        let hookable: Vec<_> = parsed.functions.iter().filter(|f| !f.is_static).collect();
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
        additive_methods = additive_methods.len(),
        "bake pass 1 complete",
    );

    // Pass 2: redirect consumer call sites on non-additive scripts.
    // Build the additive-method and additive-type sets that the
    // rewriter consults for both halves of its decision (matching method
    // name + matching receiver type).
    let additive_refs: HashSet<&str> = additive_methods.iter().map(String::as_str).collect();
    let additive_types: HashSet<&str> = ADDITIVE_TEMPLATE_SCRIPTS
        .iter()
        .filter_map(|f| f.strip_suffix(".gd"))
        .collect();

    let pack_entries: Vec<RewrittenScript> = rewritten
        .into_iter()
        .map(|p| {
            let final_source = if p.is_additive || additive_refs.is_empty() {
                p.source
            } else {
                rewrite_consumer_calls(&p.source, &p.parsed, &additive_refs, &additive_types)
            };
            RewrittenScript {
                zip_path: format!("Scripts/{}", p.filename),
                rewritten_source: final_source,
                // Resource-typed (additive) scripts must NOT ship the
                // empty `.gdc` companion, since it breaks Godot's
                // resource-script class binding for `.tres` references
                // pointing at the script's path. Node-typed scripts
                // need it to shadow vanilla PCK bytecode.
                emit_empty_gdc: !p.is_additive,
            }
        })
        .collect();

    let script_count = pack_entries.len();
    let pack = emit_pack(&PackInputs {
        rewritten_scripts: &pack_entries,
        out_path: inputs.out_pack,
        version: inputs.crabby_version,
    })?;

    info!(
        scripts = script_count,
        zero_byte_skipped,
        runtime_incompatible_skipped,
        additive_skipped,
        scripts_with_hooks = stats.scripts_with_hooks,
        total_hooks = stats.total_hooks,
        additive_scripts = stats.additive_scripts,
        additive_hooks = stats.additive_hooks,
        fast_hooks = stats.fast_hooks,
        void_hooks = stats.void_hooks,
        non_void_hooks = stats.non_void_hooks,
        data_intercept_scripts = stats.data_intercept_scripts,
        additive_method_names = stats.additive_method_names,
        "bake complete",
    );

    Ok(BakeOutputs {
        pack_path: pack.zip_path,
        script_count,
        zero_byte_skipped,
        stats,
        bake_key,
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
