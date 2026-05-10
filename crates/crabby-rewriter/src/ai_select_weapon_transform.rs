//! `AI.gd` injection for the `ai_loadouts` registry.
//!
//! Vanilla `AI.gd::SelectWeapon()` reads `self.weapons.get_children()`,
//! picks one at random, and frees the rest. There's no vanilla path for
//! adding a weapon to that pool. The transform:
//!
//!   1. Injects a one-line prelude at the top of `SelectWeapon` that
//!      calls `_rtv_apply_ai_loadouts()` BEFORE vanilla picks.
//!   2. Appends two helper functions to the script:
//!      - `_rtv_apply_ai_loadouts()`, iterates the registry's
//!        `Engine.meta` flat list, rolls per entry, instantiates
//!        matching weapon scenes, and adds them to `self.weapons`.
//!      - `_rtv_ai_category()`, derives `"Bandit"` / `"Guard"` /
//!        `"Military"` / `"Punisher"` from `self.boss` +
//!        `self.AISpawner.zone`.
//!
//! Mirrors vostok-mod-loader's `_rtv_inject_ai_registry` in
//! `src/rewriter.gd`.

use std::fmt::Write as _;
use std::sync::LazyLock;

use regex::Regex;

/// Filename that triggers this transform.
pub const AI_FILENAME: &str = "AI.gd";

/// Engine.meta key the appendix reads at runtime. Mirrored on the shim
/// side as `_AI_LOADOUTS_ENGINE_META_KEY` in `registry/ai_loadouts.gd`.
pub const AI_LOADOUTS_ENGINE_META_KEY: &str = "_rtv_ai_loadouts";

/// Matches `func SelectWeapon():` (no params) at top level.
static SELECT_WEAPON_FN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^func\s+SelectWeapon\s*\(\s*\)\s*:\s*$"#).expect("SELECT_WEAPON_FN regex")
});

/// Apply the AI.gd-side transform to `source`. Pass-through if
/// `filename` doesn't match `AI_FILENAME` or `SelectWeapon` isn't found.
#[must_use]
pub fn transform(filename: &str, source: &str, indent: &str) -> String {
    if filename != AI_FILENAME {
        return source.to_owned();
    }

    let mut out: Vec<String> = Vec::with_capacity(source.lines().count() + 32);
    let mut prelude_inserted = false;
    for line in source.lines() {
        out.push(line.to_owned());
        if !prelude_inserted && SELECT_WEAPON_FN.is_match(line) {
            for prelude in prelude_lines(indent) {
                out.push(prelude);
            }
            prelude_inserted = true;
        }
    }

    if !prelude_inserted {
        // SelectWeapon() not found, pass through to avoid corrupting
        // the file (RTV update may have shifted it; analyzer + bake
        // will still skip ai_loadouts cleanly).
        return source.to_owned();
    }

    let mut s = out.join("\n");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s.push_str(&appendix(indent));
    s
}

fn prelude_lines(indent: &str) -> Vec<String> {
    let i1 = indent;
    vec![
        format!("{i1}# --- Crabby ai_loadouts prelude ---"),
        format!("{i1}_rtv_apply_ai_loadouts()"),
    ]
}

fn appendix(indent: &str) -> String {
    let i1 = indent;
    let i2 = format!("{indent}{indent}");
    let i3 = format!("{indent}{indent}{indent}");
    let i4 = format!("{indent}{indent}{indent}{indent}");
    let mut s = String::new();
    let _ = writeln!(s);
    let _ = writeln!(s, "# --- Crabby ai_loadouts appendix ---");
    let _ = writeln!(s, "func _rtv_apply_ai_loadouts() -> void:");
    let _ = writeln!(
        s,
        "{i1}var entries: Array = Engine.get_meta(\"{AI_LOADOUTS_ENGINE_META_KEY}\", [])",
    );
    let _ = writeln!(s, "{i1}if entries.is_empty():");
    let _ = writeln!(s, "{i2}return");
    // weapons may be null on AI scenes that don't declare it (mod-
    // defined AI scenes might omit the @export). Bail rather than
    // crash.
    let _ = writeln!(s, "{i1}if weapons == null:");
    let _ = writeln!(s, "{i2}return");
    let _ = writeln!(s, "{i1}var category: String = _rtv_ai_category()");
    let _ = writeln!(s, "{i1}if category == \"\":");
    let _ = writeln!(s, "{i2}return");
    let _ = writeln!(s, "{i1}for e in entries:");
    // Defensive: entries originate from the registry validator but
    // Engine.meta is process-global so other code could in theory
    // write nonsense in. Skip silently rather than crash.
    let _ = writeln!(s, "{i2}if not (e is Dictionary):");
    let _ = writeln!(s, "{i3}continue");
    let _ = writeln!(s, "{i2}var ai_types: Array = e.get(\"ai_types\", [])");
    let _ = writeln!(s, "{i2}if not (category in ai_types):");
    let _ = writeln!(s, "{i3}continue");
    let _ = writeln!(s, "{i2}if randf() > float(e.get(\"chance\", 1.0)):");
    let _ = writeln!(s, "{i3}continue");
    let _ = writeln!(s, "{i2}if bool(e.get(\"replace\", false)):");
    let _ = writeln!(s, "{i3}for child in weapons.get_children():");
    let _ = writeln!(s, "{i4}child.queue_free()");
    let _ = writeln!(s, "{i2}var scene: PackedScene = e.get(\"weapon_scene\")");
    let _ = writeln!(s, "{i2}if scene == null:");
    let _ = writeln!(s, "{i3}continue");
    let _ = writeln!(s, "{i2}var inst: Node = scene.instantiate()");
    let _ = writeln!(s, "{i2}weapons.add_child(inst)");
    // Vanilla SelectWeapon expects every child of `weapons` to start
    // hidden; show() runs only on the picked one. Match that contract.
    let _ = writeln!(s, "{i2}if inst.has_method(\"hide\"):");
    let _ = writeln!(s, "{i3}inst.hide()");
    let _ = writeln!(s);
    let _ = writeln!(s, "func _rtv_ai_category() -> String:");
    // self.boss is set by AISpawner.CreatePools() (true for the
    // punisher in BPool, false for regular agents in APool). AISpawner
    // is the back-reference also set there. Without AISpawner the
    // zone-driven category for this AI can't be determined, so bail.
    let _ = writeln!(s, "{i1}if boss:");
    let _ = writeln!(s, "{i2}return \"Punisher\"");
    let _ = writeln!(s, "{i1}if AISpawner == null:");
    let _ = writeln!(s, "{i2}return \"\"");
    // Zone is an int (enum). AISpawner.Zone.keys() yields the string
    // form at the same index, matching the convention used by ai_types.
    let _ = writeln!(s, "{i1}var z: int = AISpawner.zone");
    let _ = writeln!(s, "{i1}var zone_keys: Array = AISpawner.Zone.keys()");
    let _ = writeln!(s, "{i1}if z < 0 or z >= zone_keys.size():");
    let _ = writeln!(s, "{i2}return \"\"");
    let _ = writeln!(s, "{i1}match zone_keys[z]:");
    let _ = writeln!(s, "{i2}\"Area05\":");
    let _ = writeln!(s, "{i3}return \"Bandit\"");
    let _ = writeln!(s, "{i2}\"BorderZone\":");
    let _ = writeln!(s, "{i3}return \"Guard\"");
    let _ = writeln!(s, "{i2}\"Vostok\":");
    let _ = writeln!(s, "{i3}return \"Military\"");
    let _ = writeln!(s, "{i2}_:");
    let _ = writeln!(s, "{i3}return \"\"");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_for_non_ai_files() {
        let src = "extends Node\n\nfunc SelectWeapon():\n\tpass\n";
        let out = transform("Other.gd", src, "\t");
        assert_eq!(out, src);
    }

    #[test]
    fn injects_prelude_after_select_weapon_signature() {
        let src = "extends Node\n\nfunc SelectWeapon():\n\tweapons.get_child_count()\n";
        let out = transform("AI.gd", src, "\t");
        let sig_pos = out.find("func SelectWeapon():").expect("sig present");
        let prelude_pos = out
            .find("Crabby ai_loadouts prelude")
            .expect("prelude present");
        let body_pos = out.find("weapons.get_child_count()").expect("body present");
        assert!(sig_pos < prelude_pos, "prelude must follow signature\n{out}");
        assert!(prelude_pos < body_pos, "prelude must precede body\n{out}");
        assert!(out.contains("_rtv_apply_ai_loadouts()"), "{out}");
    }

    #[test]
    fn appends_apply_and_category_helpers() {
        let src = "extends Node\n\nfunc SelectWeapon():\n\tpass\n";
        let out = transform("AI.gd", src, "\t");
        assert!(out.contains("func _rtv_apply_ai_loadouts() -> void:"), "{out}");
        assert!(out.contains("func _rtv_ai_category() -> String:"), "{out}");
        assert!(
            out.contains("Engine.get_meta(\"_rtv_ai_loadouts\", [])"),
            "{out}",
        );
        assert!(out.contains("AISpawner.Zone.keys()"), "{out}");
    }

    #[test]
    fn appendix_handles_all_four_categories() {
        let src = "extends Node\n\nfunc SelectWeapon():\n\tpass\n";
        let out = transform("AI.gd", src, "\t");
        assert!(out.contains("\"Bandit\""));
        assert!(out.contains("\"Guard\""));
        assert!(out.contains("\"Military\""));
        assert!(out.contains("\"Punisher\""));
    }

    #[test]
    fn no_select_weapon_means_no_changes() {
        let src = "extends Node\n\nfunc _ready():\n\tpass\n";
        let out = transform("AI.gd", src, "\t");
        assert_eq!(out, src);
    }

    #[test]
    fn four_space_indent_respected() {
        let src = "extends Node\n\nfunc SelectWeapon():\n    pass\n";
        let out = transform("AI.gd", src, "    ");
        assert!(out.contains("    _rtv_apply_ai_loadouts()"));
        assert!(!out.contains('\t'));
    }
}
