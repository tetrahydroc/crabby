//! `FishPool.gd` injection for the `fish_species` registry.
//!
//! Vanilla `FishPool.gd` is a per-scene `MeshInstance3D` with an
//! `@export var species: Array[PackedScene]` populated in the editor.
//! Its `_ready()` picks 1-10 random fish from `species` and
//! instantiates them.
//!
//! For the `fish_species` registry, a prelude is injected at the top of
//! `_ready()` that merges mod-registered species (via
//! `Engine.get_meta("_rtv_fish_species", [])`) into the local
//! `species` array before the random-spawn loop runs. Mods register
//! once and every `FishPool` instance picks it up without editor edits.
//!
//! `pool_id == "all"` (default) targets every pool; an explicit
//! `pool_id` like `"FP_2"` targets only the matching node by name.

use std::sync::LazyLock;

use regex::Regex;

/// Filename that triggers this transform.
pub const FISH_POOL_FILENAME: &str = "FishPool.gd";

/// Engine.meta key the prelude reads at runtime. Mirrored on the shim
/// side as `_FISH_ENGINE_META_KEY`.
pub const FISH_ENGINE_META_KEY: &str = "_rtv_fish_species";

/// Matches the `func _ready():` line.
static READY_FN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^func\s+_ready\s*\(\s*\)\s*:\s*$"#).expect("READY_FN regex"));

/// Apply the FishPool-side transform to `source`. Pass-through if
/// `filename` doesn't match `FISH_POOL_FILENAME` or no `_ready()` is
/// found.
#[must_use]
pub fn transform(filename: &str, source: &str, indent: &str) -> String {
    if filename != FISH_POOL_FILENAME {
        return source.to_owned();
    }

    let mut out: Vec<String> = Vec::with_capacity(source.lines().count() + 16);
    let mut prelude_inserted = false;

    for line in source.lines() {
        out.push(line.to_owned());
        if !prelude_inserted && READY_FN.is_match(line) {
            for prelude in prelude_lines(indent) {
                out.push(prelude);
            }
            prelude_inserted = true;
        }
    }

    if !prelude_inserted {
        // _ready() not found, pass through to avoid corrupting the file.
        return source.to_owned();
    }

    let mut s = out.join("\n");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

fn prelude_lines(indent: &str) -> Vec<String> {
    let i1 = indent;
    vec![
        format!("{i1}# --- Crabby fish_species merge ---"),
        format!(
            "{i1}var _rtv_fish_extras: Array = Engine.get_meta(\"{FISH_ENGINE_META_KEY}\", [])",
        ),
        format!("{i1}for _rtv_fish_e in _rtv_fish_extras:"),
        format!("{i1}{i1}if not (_rtv_fish_e is Dictionary) or not _rtv_fish_e.has(\"scene\"):"),
        format!("{i1}{i1}{i1}continue"),
        format!("{i1}{i1}var _rtv_pool_id: String = String(_rtv_fish_e.get(\"pool_id\", \"all\"))"),
        format!("{i1}{i1}if _rtv_pool_id != \"all\" and _rtv_pool_id != name:"),
        format!("{i1}{i1}{i1}continue"),
        format!("{i1}{i1}if not (_rtv_fish_e[\"scene\"] in species):"),
        format!("{i1}{i1}{i1}species.append(_rtv_fish_e[\"scene\"])"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_for_non_fishpool_files() {
        let src = "extends Node\n\nfunc _ready(): pass\n";
        let out = transform("Other.gd", src, "\t");
        assert_eq!(out, src);
    }

    #[test]
    fn injects_prelude_after_ready_signature() {
        let src = "extends MeshInstance3D\n\nfunc _ready():\n\tset_layer_mask_value(1, false)\n";
        let out = transform("FishPool.gd", src, "\t");
        let ready_pos = out.find("func _ready():").expect("found");
        let prelude_pos = out
            .find("Crabby fish_species merge")
            .expect("prelude present");
        let body_pos = out.find("set_layer_mask_value").expect("body present");
        assert!(ready_pos < prelude_pos, "prelude must follow signature\n{out}");
        assert!(prelude_pos < body_pos, "prelude must precede body\n{out}");
    }

    #[test]
    fn prelude_reads_engine_meta_and_filters_by_pool_id() {
        let src = "extends MeshInstance3D\n\nfunc _ready():\n\tpass\n";
        let out = transform("FishPool.gd", src, "\t");
        assert!(
            out.contains("Engine.get_meta(\"_rtv_fish_species\", [])"),
            "{out}",
        );
        assert!(out.contains("_rtv_pool_id != \"all\" and _rtv_pool_id != name"), "{out}");
        assert!(out.contains("species.append"), "{out}");
    }

    #[test]
    fn no_ready_means_no_changes() {
        let src = "extends MeshInstance3D\n\nfunc _physics_process(_d): pass\n";
        let out = transform("FishPool.gd", src, "\t");
        assert_eq!(out, src);
    }
}
