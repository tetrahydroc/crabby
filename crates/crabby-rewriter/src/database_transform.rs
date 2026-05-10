//! `Database.gd` const-to-dict + registry injection.
//!
//! Vanilla `Database.gd` contains hundreds of `const X = preload("...")`
//! lines that serve as compile-time globals for every scene in the
//! game's item / weapon / equipment catalog. `class_name` consumers
//! (`Database.Potato`) resolve via Godot's const table, which
//! **bypasses `_get()`**, meaning a runtime override dictionary can't
//! shadow them.
//!
//! To make these runtime-overridable, the transform:
//!
//! 1. **Moves consts into a dict.** Every top-level `const X = preload(...)`
//!    becomes an entry in `var _rtv_vanilla_scenes: Dictionary`. Consumer
//!    code that did `Database.Potato` compiled to a const-table lookup
//!    and never reached `_get`; post-transform it's a plain property
//!    read that *does* hit `_get` (because `Potato` is no longer a
//!    declared member).
//! 2. **Rewrites `get_script_constant_map()`.** The one place vanilla
//!    iterates its own const table for the @tool master-list build.
//!    After the transform the const table is empty, so the call points
//!    at `_rtv_vanilla_scenes` instead.
//! 3. **Injects the registry triple.** `_rtv_mod_scenes` (mod-registered
//!    new ids), `_rtv_override_scenes` (mod overrides of vanilla ids),
//!    and `_get()` with override -> mod -> vanilla fallthrough.
//!
//! The shim's `lib.register("scenes", ...)` writes into `_rtv_mod_scenes`
//! at runtime; `lib.override` writes into `_rtv_override_scenes`. Both
//! are plain Dictionary mutations on the live Database autoload node.
//!
//! # Why Database and not `ItemData`
//!
//! Godot's `_get(property)` is called reliably on Nodes for unknown
//! properties. Resources (like `ItemData`) have edge cases with
//! `@export var` reads bypassing `_get`. Database is a Node autoload so
//! the 3-level fallthrough works the way the docs advertise. See
//! [`crate::data_intercept`] for the Resource-side story.

use std::fmt::Write as _;
use std::sync::LazyLock;

use regex::Regex;

/// Filename that triggers this transform. Matches `ParsedScript::filename`.
pub const DATABASE_FILENAME: &str = "Database.gd";

/// Name of the dict holding vanilla scene consts after the transform.
pub const VANILLA_SCENES_VAR: &str = "_rtv_vanilla_scenes";

/// Name of the dict holding mod-registered new scene ids.
pub const MOD_SCENES_VAR: &str = "_rtv_mod_scenes";

/// Name of the dict holding mod overrides of vanilla scene ids.
pub const OVERRIDE_SCENES_VAR: &str = "_rtv_override_scenes";

/// Matches a top-level `const NAME = preload("...")` line, capturing the
/// name (group 1) and the full `preload(...)` expression (group 2).
static CONST_PRELOAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^const\s+(\w+)\s*=\s*(preload\s*\(\s*"[^"]+"\s*\))\s*$"#)
        .expect("CONST_PRELOAD regex")
});

/// Matches the one known `get_script().get_script_constant_map()` call.
/// Keeps the line's leading indent for re-emit at the same column.
static CONSTANT_MAP_CALL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\s*)(var\s+\w+\s*=\s*)get_script\(\)\.get_script_constant_map\(\)")
        .expect("CONSTANT_MAP_CALL regex")
});

/// Whether `filename` is the Database script that gets this transform.
#[must_use]
pub fn is_database_script(filename: &str) -> bool {
    filename == DATABASE_FILENAME
}

/// Apply the const-to-dict rewrite pass to raw `Database.gd` source.
///
/// Idempotent: if the source doesn't contain any matching consts
/// (e.g. the transform already ran, or this isn't Database), returns
/// the input unchanged.
#[must_use]
pub fn rewrite_database_constants(source: &str) -> String {
    let mut entries: Vec<String> = Vec::new();
    let mut out_lines: Vec<String> = Vec::new();

    for line in source.split('\n') {
        if let Some(cap) = CONST_PRELOAD.captures(line)
            && let (Some(name), Some(expr)) = (cap.get(1), cap.get(2))
        {
            entries.push(format!("\t\"{}\": {},", name.as_str(), expr.as_str()));
            continue;
        }
        // Redirect the one script-constant-map iteration at its source.
        if let Some(cap) = CONSTANT_MAP_CALL.captures(line)
            && let (Some(indent), Some(prefix)) = (cap.get(1), cap.get(2))
        {
            out_lines.push(format!(
                "{}{}{}",
                indent.as_str(),
                prefix.as_str(),
                VANILLA_SCENES_VAR,
            ));
            continue;
        }
        out_lines.push(line.to_owned());
    }

    if entries.is_empty() {
        // Nothing to rewrite. Return original (not `out_lines.join`) to
        // guarantee byte-identical output when the transform is a no-op.
        return source.to_owned();
    }

    // Inject the dict block above the first top-level `func`. If there
    // isn't one, append at end (shouldn't happen for Database but stay
    // graceful).
    let dict_block = render_vanilla_scenes_dict(&entries);
    let insert_at = out_lines
        .iter()
        .position(|l| l.starts_with("func ") || l.starts_with("static func "))
        .unwrap_or(out_lines.len());

    let mut result = String::with_capacity(source.len() + dict_block.len());
    for l in &out_lines[..insert_at] {
        result.push_str(l);
        result.push('\n');
    }
    result.push_str(&dict_block);
    let tail = &out_lines[insert_at..];
    for (j, l) in tail.iter().enumerate() {
        result.push_str(l);
        if j + 1 < tail.len() {
            result.push('\n');
        }
    }
    // Preserve trailing newline if the source had one.
    if source.ends_with('\n') && !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// Emit the registry injection block appended at end-of-script.
///
/// Uses `indent` (tab or spaces) to match the script's own style.
#[must_use]
pub fn emit_registry_injection(indent: &str) -> String {
    let i1 = indent;
    let i2 = format!("{indent}{indent}");
    let mut out = String::new();
    let _ = writeln!(out, "\n# --- Crabby scenes-registry injection ---");
    let _ = writeln!(out, "var {MOD_SCENES_VAR}: Dictionary = {{}}");
    let _ = writeln!(out, "var {OVERRIDE_SCENES_VAR}: Dictionary = {{}}");
    let _ = writeln!(out);
    let _ = writeln!(out, "func _get(property: StringName):");
    let _ = writeln!(out, "{i1}var key := String(property)");
    let _ = writeln!(out, "{i1}if {OVERRIDE_SCENES_VAR}.has(key):");
    let _ = writeln!(out, "{i2}return {OVERRIDE_SCENES_VAR}[key]");
    let _ = writeln!(out, "{i1}if {MOD_SCENES_VAR}.has(key):");
    let _ = writeln!(out, "{i2}return {MOD_SCENES_VAR}[key]");
    let _ = writeln!(out, "{i1}if {VANILLA_SCENES_VAR}.has(key):");
    let _ = writeln!(out, "{i2}return {VANILLA_SCENES_VAR}[key]");
    let _ = writeln!(out, "{i1}return null");
    out
}

fn render_vanilla_scenes_dict(entries: &[String]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "\n# --- Crabby scenes-registry: vanilla consts moved into dict ---",
    );
    let _ = writeln!(out, "var {VANILLA_SCENES_VAR}: Dictionary = {{");
    for e in entries {
        let _ = writeln!(out, "{e}");
    }
    let _ = writeln!(out, "}}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_database_filename() {
        assert!(is_database_script("Database.gd"));
        assert!(!is_database_script("database.gd")); // case-sensitive
        assert!(!is_database_script("ItemData.gd"));
        assert!(!is_database_script("Database"));
    }

    #[test]
    fn moves_top_level_const_preload_into_dict() {
        let src = "\
extends Node

const Potato = preload(\"res://Items/Potato.tscn\")
const Canned = preload(\"res://Items/Canned.tscn\")

func Foo():
\tpass
";
        let out = rewrite_database_constants(src);
        // Consts are gone.
        assert!(!out.contains("const Potato ="), "{out}");
        assert!(!out.contains("const Canned ="), "{out}");
        // Dict block is present with both entries.
        assert!(out.contains("var _rtv_vanilla_scenes: Dictionary = {"));
        assert!(
            out.contains("\"Potato\": preload(\"res://Items/Potato.tscn\")"),
            "{out}"
        );
        assert!(
            out.contains("\"Canned\": preload(\"res://Items/Canned.tscn\")"),
            "{out}"
        );
        // Dict lands above the first func.
        let dict_idx = out.find("_rtv_vanilla_scenes").unwrap();
        let func_idx = out.find("func Foo():").unwrap();
        assert!(dict_idx < func_idx, "dict should precede func");
    }

    #[test]
    fn preserves_non_preload_consts() {
        // `const X = 1` isn't a preload, should NOT be moved.
        let src = "\
extends Node

const Version = 1
const Potato = preload(\"res://Items/Potato.tscn\")

func Foo():
\tpass
";
        let out = rewrite_database_constants(src);
        assert!(out.contains("const Version = 1"));
        assert!(!out.contains("const Potato"));
    }

    #[test]
    fn idempotent_on_source_without_preload_consts() {
        let src = "extends Node\n\nfunc Foo():\n\tpass\n";
        let out = rewrite_database_constants(src);
        assert_eq!(out, src);
    }

    #[test]
    fn rewrites_script_constant_map_call() {
        let src = "\
extends Node

const Potato = preload(\"res://Items/Potato.tscn\")

func ExecuteUpdate():
\tvar constants = get_script().get_script_constant_map()
\tfor name in constants:
\t\tpass
";
        let out = rewrite_database_constants(src);
        assert!(out.contains("var constants = _rtv_vanilla_scenes"), "{out}");
        assert!(
            !out.contains("get_script().get_script_constant_map()"),
            "{out}"
        );
    }

    #[test]
    fn emit_registry_injection_has_expected_shape_with_tabs() {
        let out = emit_registry_injection("\t");
        assert!(out.contains("var _rtv_mod_scenes: Dictionary = {}"));
        assert!(out.contains("var _rtv_override_scenes: Dictionary = {}"));
        assert!(out.contains("func _get(property: StringName):"));
        // 3-level fallthrough in the right order: override > mod > vanilla.
        let over_idx = out.find("_rtv_override_scenes.has(key)").unwrap();
        let mod_idx = out.find("_rtv_mod_scenes.has(key)").unwrap();
        let van_idx = out.find("_rtv_vanilla_scenes.has(key)").unwrap();
        assert!(over_idx < mod_idx);
        assert!(mod_idx < van_idx);
        // Tab indent respected.
        assert!(out.contains("\tvar key := String(property)"));
        assert!(out.contains("\t\treturn _rtv_override_scenes[key]"));
    }

    #[test]
    fn emit_registry_injection_respects_four_space_indent() {
        let out = emit_registry_injection("    ");
        assert!(out.contains("    var key := String(property)"));
        assert!(out.contains("        return _rtv_override_scenes[key]"));
        assert!(!out.contains('\t'));
    }

    #[test]
    fn indented_consts_are_not_touched() {
        // `const` inside an inner class or indented scope must stay put.
        // Only top-level (column 0) consts are preload-extractable; the
        // regex anchors on `^const` which requires column 0.
        let src = "\
extends Node

class Inner:
\tconst Nested = preload(\"res://x.tscn\")

const TopLevel = preload(\"res://y.tscn\")

func Foo():
\tpass
";
        let out = rewrite_database_constants(src);
        // Top-level moved.
        assert!(out.contains("\"TopLevel\": preload(\"res://y.tscn\")"));
        // Inner-class const stayed put.
        assert!(out.contains("\tconst Nested = preload(\"res://x.tscn\")"));
    }

    #[test]
    fn trailing_newline_preserved() {
        let src = "extends Node\nconst A = preload(\"res://a.tscn\")\nfunc B():\n\tpass\n";
        let out = rewrite_database_constants(src);
        assert!(out.ends_with('\n'));
    }
}
