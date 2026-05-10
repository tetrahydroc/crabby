//! Full-script orchestrator: every hookable method on a vanilla script
//! gets a dispatch wrapper, and pure-data resource scripts get a
//! `_get` override for registry patch interception.
//!
//! Rename prefix is `_rtv_vanilla_`; hookable = every `func` that is
//! **not** `static`. Template is picked by
//! [`pick_template`](crate::pick_template), see
//! [`TemplateKind`](crate::TemplateKind) for the set of templates and
//! the selection precedence.
//!
//! # Coroutine handling
//!
//! Coroutine methods (those with `await` in the body,
//! [`FuncDecl::is_coroutine`](crabby_parser::FuncDecl::is_coroutine)) do
//! **not** require a separate template. Each existing template emits
//! `await ` before every vanilla-call site when `is_coroutine` is true,
//! making the wrapper itself a coroutine that yields at each call into
//! the renamed vanilla body. `Message.gd` and `Explosion.gd` (vostok
//! skip-list entries for "await-based `_ready` breaks dispatch") are
//! hooked via this mechanism, the per-template unit tests pin the
//! "every vanilla-call site is awaited" invariant.
//!
//! # Data-intercept handling
//!
//! Pure-data `Resource` scripts ([`DATA_INTERCEPT_SCRIPTS`](crate::data_intercept::DATA_INTERCEPT_SCRIPTS))
//! have no methods to wrap. Instead, crabby appends a `_get(property)`
//! override plus a runtime override dict so the registry's `patch(...)`
//! verb can shadow `@export var` values at read time without changing
//! any consumer call site. This pass runs in addition to (not instead
//! of) method wrapping, so a data-intercept script that somehow grew a
//! method in a future RTV update would still get both treatments.
//!
//! # The per-script pipeline
//!
//! 1. Normalize line endings to LF.
//! 2. Detect the existing indent unit (tab or N-space) so appended
//!    wrappers match (`GDScript` rejects tab/space mixing in one file).
//! 3. Defensive autofix: inject `pass` into bodyless blocks. No-op on
//!    clean vanilla; guards against future legacy-syntax regressions.
//! 4. For non-additive scripts: rename every hookable method's top-level
//!    `func <name>(` to `func _rtv_vanilla_<name>(`, rewriting bare
//!    `super()` inside each.
//! 5. Append a dispatch wrapper per hookable method, chosen by
//!    [`pick_template`](crate::pick_template).
//! 6. If the script is data-intercept, append the `_get` override block.

use std::fmt::Write as _;

use crabby_error::Result;
use crabby_parser::ParsedScript;

use crate::autofix::inject_pass_into_bodyless_blocks;
// Data-intercept injection (var _rtv_mod_patches + _get override) was
// removed after empirical confirmation that Godot's _get is bypassed
// for declared @export reads. The registry now mutates live Resource
// fields directly via set(). The data_intercept module is kept (the
// kind list still feeds template selection diagnostics) but the
// per-script injection no longer fires.
#[allow(unused_imports)]
use crate::data_intercept::should_inject_data_intercept;
use crate::database_transform::{
    emit_registry_injection as emit_db_registry, is_database_script, rewrite_database_constants,
};
use crate::hook_name::{hook_base, script_prefix};
use crate::normalize::{detect_indent_style, normalize_line_endings};
use crate::renamer::rename_many;
use crate::resource_serialized::is_additive_script;
use crate::template::{additive, fast, non_void, void};
use crate::template_selection::{TemplateKind, pick_template};

/// Rewrite every hookable method on a parsed script using the standard
/// dispatch-wrapper template, plus (for data-intercept scripts) append
/// a `_get(property)` override.
///
/// # Errors
///
/// Currently infallible in practice; returns [`crabby_error::Result`]
/// to keep the API stable as later phases add template-selection errors
/// (unknown skip-list entry, per-method validation, etc.).
pub fn rewrite_full_script(source: &str, parsed: &ParsedScript) -> Result<String> {
    rewrite_full_script_with_hooks(source, parsed, None)
}

/// Like [`rewrite_full_script`] but the caller supplies a per-base map
/// of which hook KINDS the active mod profile registers
/// (`pre`/`post`/`callback`/`replace`).
///
/// Behavior:
///   * `None`: legacy, every hookable method gets a wrapper with all
///     four dispatch sites (used by tests + callers without analyzer
///     data).
///   * `Some(map)`: methods absent from the map get no wrapper at
///     all (AOT skip). Methods present get a wrapper whose
///     emitted dispatch sites are restricted to the kinds set in
///     [`HookFlags`]. When `replace = false` the entire
///     `_get_hooks` / super-skip branch is dropped (call vanilla
///     direct).
pub fn rewrite_full_script_with_hooks(
    source: &str,
    parsed: &ParsedScript,
    hooked_kinds: Option<&std::collections::HashMap<String, crate::HookFlags>>,
) -> Result<String> {
    // Pre-filter the hookable list when analyzer data is present:
    // only methods that ARE actually hooked by some active mod get
    // wrappers. Methods nothing hooks pass through vanilla, no
    // dispatcher overhead at runtime, smaller PCK.
    let prefix = script_prefix(&parsed.filename);
    let hookable: Vec<_> = parsed
        .functions
        .iter()
        .filter(|f| !f.is_static)
        .filter(|f| match hooked_kinds {
            None => true,
            Some(map) => map.contains_key(&hook_base(&prefix, &f.name)),
        })
        .collect();
    let inject_db_scenes = is_database_script(&parsed.filename);
    let inject_loader = parsed.filename == crate::loader_transform::LOADER_FILENAME;
    let inject_ai_spawner = parsed.filename == crate::ai_spawner_transform::AI_SPAWNER_FILENAME;
    let inject_ai = parsed.filename == crate::ai_select_weapon_transform::AI_FILENAME;
    let inject_fish_pool = parsed.filename == crate::fish_pool_transform::FISH_POOL_FILENAME;
    let inject_compiler = parsed.filename == crate::compiler_spawn_transform::COMPILER_FILENAME;
    let inject_recipes_index =
        parsed.filename == crate::recipes_index_transform::RECIPES_SCHEMA_FILENAME;
    let inject_events_index =
        parsed.filename == crate::events_index_transform::EVENTS_SCHEMA_FILENAME;
    let inject_loot_table_index =
        parsed.filename == crate::loot_table_index_transform::LOOT_TABLE_SCHEMA_FILENAME;
    let inject_trader_data_index =
        parsed.filename == crate::trader_data_index_transform::TRADER_DATA_SCHEMA_FILENAME;

    // Early return when the script has nothing to rewrite AND isn't a
    // target for any per-script injection. Data-intercept scripts with
    // no hookable methods pass through unchanged: direct set() works
    // on their @export fields without any shim.
    if hookable.is_empty()
        && !inject_db_scenes
        && !inject_loader
        && !inject_ai_spawner
        && !inject_ai
        && !inject_fish_pool
        && !inject_compiler
        && !inject_recipes_index
        && !inject_events_index
        && !inject_loot_table_index
        && !inject_trader_data_index
    {
        return Ok(normalize_line_endings(source));
    }

    let normalized = normalize_line_endings(source);
    let indent = detect_indent_style(&normalized);

    // Per-script transforms run BEFORE method rename / wrapper emission
    // so injected blocks land in the right structural position
    // (declarations at module scope, preludes inside functions that
    // will become `_rtv_vanilla_<method>` after rename).
    let source_for_autofix = if inject_db_scenes {
        rewrite_database_constants(&normalized)
    } else if inject_loader {
        crate::loader_transform::transform(&parsed.filename, &normalized, &indent)
    } else if inject_ai_spawner {
        crate::ai_spawner_transform::transform(&parsed.filename, &normalized)
    } else if inject_ai {
        crate::ai_select_weapon_transform::transform(&parsed.filename, &normalized, &indent)
    } else if inject_fish_pool {
        crate::fish_pool_transform::transform(&parsed.filename, &normalized, &indent)
    } else if inject_compiler {
        crate::compiler_spawn_transform::transform(&parsed.filename, &normalized, &indent)
    } else if inject_recipes_index {
        // Append id-index machinery (var _id_index, _init builder, and
        // _index_add/remove/set helpers) to vanilla Recipes.gd.
        // See recipes_index_transform module.
        crate::recipes_index_transform::transform(&parsed.filename, &normalized)
    } else if inject_events_index {
        // Same pattern as recipes for vanilla Events.gd. Single
        // typed-Array (`events`) instead of seven categorized ones.
        crate::events_index_transform::transform(&parsed.filename, &normalized)
    } else if inject_loot_table_index {
        // LootTable is the shared schema for LT_Master + every loot
        // table. Index keyed by ItemData.file (the canonical id).
        crate::loot_table_index_transform::transform(&parsed.filename, &normalized)
    } else if inject_trader_data_index {
        // TraderData is the shared schema for the four trader resources.
        // Index keyed by file-stem of each task.resource_path.
        crate::trader_data_index_transform::transform(&parsed.filename, &normalized)
    } else {
        normalized
    };

    // Defensive autofix: inject `pass` into bodyless blocks. No-op on
    // clean vanilla (full-corpus scan reports zero injections on RTV
    // 4.6.1); the pass exists so a future RTV update introducing
    // accidental bodyless blocks doesn't cascade parse errors through
    // the hook pack. Runs before rename so the scan sees the original
    // function names (the scan doesn't actually care, but this keeps
    // the pipeline ordering easy to reason about).
    let (fixed, injections) = inject_pass_into_bodyless_blocks(&source_for_autofix, &indent);
    if injections > 0 {
        tracing::debug!(
            script = %parsed.filename,
            injections,
            "autofix: injected `pass` into bodyless blocks",
        );
    }

    // Decide whether to run the rename pass:
    // - Skip for additive-template scripts: their methods keep their
    //   original names so persisted saves still match the script shape.
    // - Skip when there are no hookable methods (nothing to rename).
    // - Otherwise, rename every hookable method's top-level declaration.
    let skip_rename = is_additive_script(&parsed.filename) || hookable.is_empty();
    let body_for_wrappers: String = if skip_rename {
        fixed
    } else {
        let target_names: Vec<&str> = hookable.iter().map(|f| f.name.as_str()).collect();
        rename_many(&fixed, &target_names, "_rtv_vanilla_")
    };

    let mut out = String::with_capacity(body_for_wrappers.len() + hookable.len() * 512 + 512);
    out.push_str(&body_for_wrappers);
    if !out.ends_with('\n') {
        out.push('\n');
    }

    // Wrapper append pass (skipped when no hookable methods).
    if !hookable.is_empty() {
        out.push_str("\n# --- Crabby hook dispatch wrappers ---\n");
        for func in hookable {
            // Default flags = all kinds enabled (legacy "wrap everything"
            // behavior). When the caller supplied analyzer data, look up
            // the actual kinds set for this base; the wrapper templates
            // will elide dispatch sites for kinds nobody hooks (B2.2).
            let flags = match hooked_kinds {
                None => crate::HookFlags::all(),
                Some(map) => map
                    .get(&hook_base(&prefix, &func.name))
                    .copied()
                    .unwrap_or(crate::HookFlags::all()),
            };
            let inputs = void::Inputs {
                func,
                script_prefix: &prefix,
                indent: &indent,
                rename_prefix: "_rtv_vanilla_",
                flags,
            };
            let wrapper = match pick_template(parsed, func) {
                TemplateKind::Additive => additive::emit(&inputs),
                TemplateKind::Fast => fast::emit(&inputs),
                TemplateKind::Void => void::emit(&inputs),
                TemplateKind::NonVoid => non_void::emit(&inputs),
            };
            let _ = writeln!(out, "{wrapper}");
        }
    }

    // Scenes-registry injection for Database. The `_rtv_vanilla_scenes`
    // dict was emitted earlier by `rewrite_database_constants` above; this
    // appends the mod / override dicts and the 3-level `_get()`.
    if inject_db_scenes {
        out.push_str(&emit_db_registry(&indent));
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabby_parser::parse_script;

    fn parse(filename: &str, src: &str) -> ParsedScript {
        parse_script(filename, src).expect("parse")
    }

    #[test]
    fn empty_script_is_no_op_after_normalization() {
        let src = "extends Node\r\nclass_name Foo\r\n";
        let parsed = parse("Foo.gd", src);
        let out = rewrite_full_script(src, &parsed).unwrap();
        // Line endings normalized even though nothing was wrapped.
        assert!(!out.contains('\r'));
        assert!(!out.contains("Crabby hook dispatch wrappers"));
    }

    #[test]
    fn void_method_uses_void_template() {
        let src = "\
extends Node

func _physics_process(delta):
\tpass
";
        let parsed = parse("Controller.gd", src);
        let out = rewrite_full_script(src, &parsed).unwrap();
        // Renamed body present.
        assert!(out.contains("func _rtv_vanilla__physics_process(delta):"));
        // Void wrapper: no `var _result`, no `return _result`.
        assert!(!out.contains("var _result"));
        // Dispatch calls present.
        assert!(out.contains("controller-_physics_process-pre"));
    }

    #[test]
    fn non_void_method_uses_non_void_template() {
        let src = "\
extends Node

func calc(x) -> int:
\treturn x + 1
";
        let parsed = parse("Calc.gd", src);
        let out = rewrite_full_script(src, &parsed).unwrap();
        assert!(out.contains("func _rtv_vanilla_calc(x) -> int:"));
        assert!(out.contains("var _result"));
        assert!(out.ends_with("return _result\n\n") || out.contains("return _result\n"));
    }

    #[test]
    fn engine_lifecycle_overrides_body_inference() {
        // Body has `return 1` but `_ready` is always void, void template
        // must win so the wrapper compiles against Godot's expected sig.
        let src = "\
extends Node

func _ready():
\treturn 1
";
        let parsed = parse("X.gd", src);
        let out = rewrite_full_script(src, &parsed).unwrap();
        assert!(!out.contains("var _result"));
        assert!(out.contains("x-_ready-pre"));
    }

    #[test]
    fn static_methods_are_skipped() {
        let src = "\
extends Node

static func util() -> int:
\treturn 1

func ordinary():
\tpass
";
        let parsed = parse("X.gd", src);
        let out = rewrite_full_script(src, &parsed).unwrap();
        // `util` stays as-is; no wrapper emitted for it.
        assert!(out.contains("static func util() -> int:"));
        assert!(!out.contains("x-util"));
        // `ordinary` was wrapped.
        assert!(out.contains("func _rtv_vanilla_ordinary():"));
        assert!(out.contains("x-ordinary"));
    }

    #[test]
    fn crlf_source_normalized_before_rewrite() {
        let src = "extends Node\r\n\r\nfunc foo():\r\n\tpass\r\n";
        let parsed = parse("X.gd", src);
        let out = rewrite_full_script(src, &parsed).unwrap();
        assert!(!out.contains('\r'));
    }

    #[test]
    fn four_space_indent_preserved_in_wrappers() {
        let src = "\
extends Node

func foo():
    pass
";
        let parsed = parse("X.gd", src);
        let out = rewrite_full_script(src, &parsed).unwrap();
        // Wrapper uses 4-space indent throughout (no tabs).
        // Find the wrapper's `var _lib` line and check its indent.
        let wrapper_line = out
            .lines()
            .find(|l| l.contains("var _lib = Engine.get_meta"))
            .expect("wrapper _lib line present");
        assert!(wrapper_line.starts_with("    ") && !wrapper_line.starts_with('\t'));
    }

    #[test]
    fn multiple_methods_all_wrapped() {
        let src = "\
extends Node

func a():
\tpass

func b():
\tpass

func c() -> int:
\treturn 1
";
        let parsed = parse("X.gd", src);
        let out = rewrite_full_script(src, &parsed).unwrap();
        assert!(out.contains("func _rtv_vanilla_a():"));
        assert!(out.contains("func _rtv_vanilla_b():"));
        assert!(out.contains("func _rtv_vanilla_c() -> int:"));
        assert!(out.contains("x-a-pre"));
        assert!(out.contains("x-b-pre"));
        assert!(out.contains("x-c-pre"));
    }

    #[test]
    fn data_intercept_script_with_no_methods_gets_injection() {
        // Pure-data Resource script with no hookable methods. Since the
        // data-intercept _get shim was removed (it can't intercept
        // declared @export reads anyway), the script passes through
        // unchanged.
        let src = "\
extends Resource
class_name ItemData

@export var name: String
@export var weight := 1.0
";
        let parsed = parse("ItemData.gd", src);
        let out = rewrite_full_script(src, &parsed).unwrap();
        // Original decls preserved.
        assert!(out.contains("extends Resource"));
        assert!(out.contains("@export var name: String"));
        assert!(out.contains("@export var weight := 1.0"));
        // No injection: the dict + _get shim are gone.
        assert!(!out.contains("data-intercept injection"));
        assert!(!out.contains("_rtv_mod_patches"));
        assert!(!out.contains("func _get(property:"));
        // No dispatch wrappers (no methods to wrap).
        assert!(!out.contains("Crabby hook dispatch wrappers"));
        assert!(!out.contains("_lib._dispatch"));
    }

    #[test]
    fn ordinary_script_does_not_get_data_intercept_injection() {
        let src = "\
extends Node

func foo():
\tpass
";
        let parsed = parse("Controller.gd", src);
        let out = rewrite_full_script(src, &parsed).unwrap();
        assert!(!out.contains("data-intercept injection"));
        assert!(!out.contains("_rtv_mod_patches"));
        assert!(!out.contains("func _get(property:"));
    }

    #[test]
    fn data_intercept_script_passes_through_unchanged() {
        // Data-intercept scripts get no injection at all. The
        // rewriter must leave them byte-equivalent (modulo line-ending
        // normalization). Registry mutates @export fields directly.
        let src = "\
extends Resource
class_name ItemData

@export var name: String
";
        let parsed = parse("ItemData.gd", src);
        let out = rewrite_full_script(src, &parsed).unwrap();
        assert!(!out.contains("Crabby data-intercept"));
        assert!(!out.contains("_rtv_mod_patches"));
    }

    #[test]
    fn database_gets_both_const_move_and_registry_injection() {
        // Integration: Database should land with the vanilla-scenes dict
        // ABOVE its first func (const-to-dict pass), and the 3-level
        // registry `_get` appended AFTER all wrappers (scenes-registry
        // injection). The original const lines must be gone and the
        // dict must hold the original preload expression.
        let src = "\
@tool
extends Node

const Potato = preload(\"res://Items/Potato.tscn\")
const Can_Empty = preload(\"res://Items/Can_Empty.tscn\")

func ExecuteUpdate(_value: bool) -> void:
\tvar constants = get_script().get_script_constant_map()
\tprint(constants)
";
        let parsed = parse("Database.gd", src);
        let out = rewrite_full_script(src, &parsed).unwrap();

        // Vanilla consts are gone; dict contains them.
        assert!(!out.contains("const Potato "), "const stayed: {out}");
        assert!(
            out.contains("\"Potato\": preload(\"res://Items/Potato.tscn\")"),
            "missing dict entry: {out}"
        );
        // Script-constant-map call was redirected.
        assert!(out.contains("var constants = _rtv_vanilla_scenes"), "{out}");
        assert!(!out.contains("get_script_constant_map"), "{out}");
        // Registry triple is present.
        assert!(out.contains("var _rtv_mod_scenes: Dictionary = {}"));
        assert!(out.contains("var _rtv_override_scenes: Dictionary = {}"));
        assert!(out.contains("func _get(property: StringName):"));
        // Vanilla dict must appear BEFORE the func, registry injection
        // AFTER the func wrappers.
        let vanilla_idx = out.find("var _rtv_vanilla_scenes").unwrap();
        let func_idx = out.find("func _rtv_vanilla_ExecuteUpdate").unwrap();
        let registry_idx = out.find("# --- Crabby scenes-registry injection").unwrap();
        assert!(vanilla_idx < func_idx, "dict should precede funcs");
        assert!(registry_idx > func_idx, "registry should follow funcs");
    }

    #[test]
    fn aot_skip_passes_through_unhooked_methods() {
        // With an empty hook set, no method gets wrapped, the
        // output is essentially vanilla (modulo line-ending
        // normalization). No `_rtv_vanilla_` prefix anywhere.
        let src = "\
extends Node

func ApplyDamage(dmg: float) -> void:
\tpass

func Heal(amt: float) -> void:
\tpass
";
        let parsed = parse("Hitbox.gd", src);
        let empty: std::collections::HashMap<String, crate::HookFlags> =
            std::collections::HashMap::new();
        let out = rewrite_full_script_with_hooks(src, &parsed, Some(&empty)).unwrap();
        // No wrappers emitted.
        assert!(!out.contains("_rtv_vanilla_ApplyDamage"), "{out}");
        assert!(!out.contains("_rtv_vanilla_Heal"), "{out}");
        assert!(!out.contains("Crabby hook dispatch wrappers"), "{out}");
    }

    #[test]
    fn aot_skip_wraps_only_hooked_methods() {
        // Only ApplyDamage is in the hook set, only it gets wrapped.
        let src = "\
extends Node

func ApplyDamage(dmg: float) -> void:
\tpass

func Heal(amt: float) -> void:
\tpass
";
        let parsed = parse("Hitbox.gd", src);
        let mut map: std::collections::HashMap<String, crate::HookFlags> =
            std::collections::HashMap::new();
        map.insert("hitbox-applydamage".into(), crate::HookFlags::all());
        let out = rewrite_full_script_with_hooks(src, &parsed, Some(&map)).unwrap();
        // ApplyDamage gets the rename + wrapper.
        assert!(out.contains("_rtv_vanilla_ApplyDamage"), "{out}");
        // Heal stays vanilla (no rename, no wrapper).
        assert!(!out.contains("_rtv_vanilla_Heal"), "{out}");
        // Wrapper section header appears (at least one was emitted).
        assert!(out.contains("Crabby hook dispatch wrappers"), "{out}");
    }

    #[test]
    fn aot_kinds_pre_only_elides_other_dispatch_sites() {
        // Method base is in the map with only `pre` set, wrapper
        // appears, but its emitted body has no -post / -callback and
        // no replace-probe block.
        let src = "\
extends Node

func ApplyDamage(dmg: float) -> void:
\tpass
";
        let parsed = parse("Hitbox.gd", src);
        let mut map: std::collections::HashMap<String, crate::HookFlags> =
            std::collections::HashMap::new();
        map.insert(
            "hitbox-applydamage".into(),
            crate::HookFlags { pre: true, ..Default::default() },
        );
        let out = rewrite_full_script_with_hooks(src, &parsed, Some(&map)).unwrap();
        // Wrapper emitted (rename happened, dispatcher present).
        assert!(out.contains("_rtv_vanilla_ApplyDamage"), "{out}");
        // Only -pre dispatch line landed.
        assert!(out.contains("hitbox-applydamage-pre"), "{out}");
        assert!(!out.contains("hitbox-applydamage-post"), "{out}");
        assert!(!out.contains("hitbox-applydamage-callback"), "{out}");
        // No replace-probe scaffolding.
        assert!(!out.contains("_get_hooks"), "{out}");
    }

    #[test]
    fn aot_none_keeps_legacy_wrap_everything() {
        // When hook_set is None (legacy callers), every hookable
        // method still gets wrapped, same behavior as before.
        let src = "\
extends Node

func Foo() -> void:
\tpass
";
        let parsed = parse("Bar.gd", src);
        let out = rewrite_full_script_with_hooks(src, &parsed, None).unwrap();
        assert!(out.contains("_rtv_vanilla_Foo"), "{out}");
    }

    #[test]
    fn non_database_scripts_never_get_scenes_registry() {
        let src = "\
extends Node

const Foo = preload(\"res://x.tscn\")

func Bar():
\tpass
";
        let parsed = parse("SomeOtherScript.gd", src);
        let out = rewrite_full_script(src, &parsed).unwrap();
        // The const-move + registry are Database-only.
        assert!(
            out.contains("const Foo = preload("),
            "should NOT move: {out}"
        );
        assert!(!out.contains("_rtv_mod_scenes"));
        assert!(!out.contains("_rtv_vanilla_scenes"));
    }
}
