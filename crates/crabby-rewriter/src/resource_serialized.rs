//! Resource-serialized script list, scripts whose method names are
//! embedded in save files by `ResourceSaver` and therefore can't be
//! renamed without breaking save compatibility.
//!
//! Port of vostok-mod-loader's `RTV_RESOURCE_SERIALIZED_SKIP` from
//! `constants.gd`. Vostok skips hooks on these scripts entirely; crabby
//! uses the [additive template](crate::template::additive) instead so
//! hooks still fire without renaming the vanilla body.

/// Vanilla `Resource`-subclass scripts whose method names are load-bearing
/// in persisted save files. Each filename includes the `.gd` suffix to
/// match the exact shape of `ParsedScript::filename`.
///
/// When a script in this list is rewritten, the vanilla methods keep
/// their original names so existing saves still deserialize correctly;
/// dispatch wrappers are emitted under a distinct
/// [`ADDITIVE_HOOK_PREFIX`]. Consumer scripts elsewhere in the corpus
/// get their call sites rewritten to call the hooked variant, see
/// [`rewrite_consumer_calls`](crate::rewrite_consumer_calls).
pub const ADDITIVE_TEMPLATE_SCRIPTS: &[&str] = &[
    "CharacterSave.gd",
    "ContainerSave.gd",
    "FurnitureSave.gd",
    "ItemSave.gd",
    "Preferences.gd",
    "ShelterSave.gd",
    "SlotData.gd",
    "SwitchSave.gd",
    "TraderSave.gd",
    "Validator.gd",
    "WorldSave.gd",
];

/// Prefix used on additive wrapper methods. Chosen to be distinct from
/// `_rtv_vanilla_` (used by the standard rename-body templates) so the
/// two conventions can't collide at the script level.
pub const ADDITIVE_HOOK_PREFIX: &str = "_rtv_hooked_";

/// Whether `filename` is a resource-serialized script that must be
/// rewritten with the additive template.
#[must_use]
pub fn is_additive_script(filename: &str) -> bool {
    ADDITIVE_TEMPLATE_SCRIPTS.contains(&filename)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    fn list_entries_end_with_gd() {
        // Godot resource paths are case-sensitive; the lint's preferred
        // case-insensitive variant would be wrong for this domain.
        for name in ADDITIVE_TEMPLATE_SCRIPTS {
            assert!(
                name.ends_with(".gd"),
                "ADDITIVE_TEMPLATE_SCRIPTS entries must include .gd: {name}",
            );
        }
    }

    #[test]
    fn is_additive_script_exact_match() {
        assert!(is_additive_script("WorldSave.gd"));
        assert!(is_additive_script("CharacterSave.gd"));
        assert!(!is_additive_script("worldsave.gd")); // case-sensitive
        assert!(!is_additive_script("WorldSave")); // extension required
        assert!(!is_additive_script("Controller.gd"));
    }

    #[test]
    fn additive_hook_prefix_is_distinct_from_vanilla_rename() {
        // The standard template renames vanilla bodies to `_rtv_vanilla_`.
        // The additive template leaves bodies named and adds wrappers
        // under a different prefix so the two conventions can't collide.
        assert_ne!(ADDITIVE_HOOK_PREFIX, "_rtv_vanilla_");
    }
}
