//! Data-intercept script list, pure-data `Resource`-subclass scripts
//! whose `@export var` fields are the entire API surface.
//!
//! These scripts have no methods to wrap (confirmed across the full RTV
//! 4.6.1 corpus: all 25 entries have zero `func` declarations). Instead
//! of a dispatch-wrapper template, crabby injects a `_get(property)`
//! override that routes field reads through the registry's patch
//! dictionary.
//!
//! The registry API (`lib.patch("items", "Pistol", {damage: 50})`)
//! populates the injected `_rtv_mod_patches` dict at runtime; Godot's
//! property-resolution chain consults `_get` when the requested name
//! isn't a declared property or const, so patched values transparently
//! override the exported defaults without touching call sites.
//!
//! Port of vostok-mod-loader's `RTV_RESOURCE_DATA_SKIP`. Vostok skips
//! these entirely ("no hook point needed"); crabby keeps them hookable
//! via the registry API at no per-call cost when no patch is active.

/// Pure-data `Resource`-subclass scripts receiving the data-intercept
/// treatment. Each filename includes the `.gd` suffix to match the exact
/// shape of `ParsedScript::filename`.
pub const DATA_INTERCEPT_SCRIPTS: &[&str] = &[
    "AIWeaponData.gd",
    "AttachmentData.gd",
    "AudioEvent.gd",
    "AudioLibrary.gd",
    "CasetteData.gd",
    "CatData.gd",
    "EventData.gd",
    "Events.gd",
    "FishingData.gd",
    "FurnitureData.gd",
    "GrenadeData.gd",
    "InstrumentData.gd",
    "ItemData.gd",
    "KnifeData.gd",
    "LootTable.gd",
    "RecipeData.gd",
    "Recipes.gd",
    "SpawnerChunkData.gd",
    "SpawnerData.gd",
    "SpawnerSceneData.gd",
    "SpineData.gd",
    "TaskData.gd",
    "TrackData.gd",
    "TraderData.gd",
    "WeaponData.gd",
];

/// Name of the runtime override dictionary injected by the data-intercept
/// template. The registry API populates this; `_get` consults it first
/// so patched fields shadow their exported defaults.
pub const PATCH_DICT_VAR_NAME: &str = "_rtv_mod_patches";

/// Whether `filename` is a data-intercept target.
#[must_use]
pub fn is_data_intercept_script(filename: &str) -> bool {
    DATA_INTERCEPT_SCRIPTS.contains(&filename)
}

/// Whether the data-intercept injection should actually run on this
/// script.
///
/// True iff the script is in [`DATA_INTERCEPT_SCRIPTS`] **and** does not
/// extend another data-intercept target. The `extends` exclusion exists
/// because Godot rejects re-declaring a `var` already declared in the
/// parent class. Three RTV 4.6.1 scripts form an inheritance chain
/// (`SpawnerData` <- `SpawnerChunkData`/`SpawnerSceneData`), and injecting
/// `var _rtv_mod_patches` into both the parent and the child caused
/// "member already exists in parent class" parse errors that cascaded
/// through `Spawner.gd` and silently nulled out item rendering at
/// runtime.
///
/// Children inherit the parent's `_rtv_mod_patches` field and `_get`
/// override automatically, so skipping the redundant injection is also
/// semantically correct: the registry patch dict on the instance is
/// shared with the parent, and `_get` resolution still fires for the
/// child's own properties via the inherited override.
#[must_use]
pub fn should_inject_data_intercept(filename: &str, extends: Option<&str>) -> bool {
    if !is_data_intercept_script(filename) {
        return false;
    }
    let Some(parent) = extends else {
        return true;
    };
    // `class_name X` lives in `X.gd` for every data-intercept script in
    // RTV 4.6.1 (verified against the live PCK). Match parent class name
    // against `<intercept_filename without .gd>`.
    !DATA_INTERCEPT_SCRIPTS
        .iter()
        .any(|f| f.strip_suffix(".gd") == Some(parent))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    fn list_entries_end_with_gd() {
        // Godot resource paths are case-sensitive; the lint's preferred
        // case-insensitive variant would be wrong for this domain.
        for name in DATA_INTERCEPT_SCRIPTS {
            assert!(
                name.ends_with(".gd"),
                "DATA_INTERCEPT_SCRIPTS entries must include .gd: {name}",
            );
        }
    }

    #[test]
    fn detects_known_data_scripts() {
        assert!(is_data_intercept_script("ItemData.gd"));
        assert!(is_data_intercept_script("LootTable.gd"));
        assert!(is_data_intercept_script("WeaponData.gd"));
    }

    #[test]
    fn rejects_non_data_scripts() {
        assert!(!is_data_intercept_script("Controller.gd"));
        assert!(!is_data_intercept_script("WorldSave.gd")); // additive, not data
        assert!(!is_data_intercept_script("itemdata.gd")); // case-sensitive
        assert!(!is_data_intercept_script("ItemData")); // extension required
    }

    #[test]
    fn patch_dict_var_name_is_distinct_from_other_injections() {
        // Just guard against an accidental collision with other crabby-
        // injected identifiers. Hardcoded check so a rename surfaces
        // here.
        assert_eq!(PATCH_DICT_VAR_NAME, "_rtv_mod_patches");
    }

    #[test]
    fn should_inject_root_intercept_script() {
        // SpawnerData has no data-intercept parent, inject.
        assert!(should_inject_data_intercept(
            "SpawnerData.gd",
            Some("Resource")
        ));
        assert!(should_inject_data_intercept(
            "ItemData.gd",
            Some("Resource")
        ));
        assert!(should_inject_data_intercept("ItemData.gd", None));
    }

    #[test]
    fn should_skip_intercept_child_of_intercept_parent() {
        // Real RTV 4.6.1 inheritance: SpawnerSceneData/SpawnerChunkData
        // both extend SpawnerData. Without this skip, a
        // "_rtv_mod_patches already exists in parent class" parse error fires.
        assert!(!should_inject_data_intercept(
            "SpawnerSceneData.gd",
            Some("SpawnerData"),
        ));
        assert!(!should_inject_data_intercept(
            "SpawnerChunkData.gd",
            Some("SpawnerData"),
        ));
    }

    #[test]
    fn should_not_inject_non_intercept_script() {
        assert!(!should_inject_data_intercept("Controller.gd", Some("Node")));
        assert!(!should_inject_data_intercept("Controller.gd", None));
    }
}
