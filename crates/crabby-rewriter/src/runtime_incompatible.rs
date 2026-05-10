//! Scripts whose source can't be re-compiled at runtime, so no
//! rewritten version is shipped and vanilla `.gdc` wins via VFS
//! fallthrough.
//!
//! # The compile-time vs runtime symbol gap
//!
//! Vanilla `.gdc` files ship pre-compiled bytecode. Some of them were
//! compiled at editor-export time and reference editor-only symbols
//! (`EditorInterface`, etc.) that simply don't exist in the exported
//! game's runtime. The bytecode is fine, the symbols are referenced
//! but never *resolved* at runtime because the call sites are guarded
//! by `Engine.is_editor_hint()`. But the pipeline detokenizes
//! `.gdc` -> text source and lets Godot re-compile that source at runtime,
//! which trips the parse-time scope check ("Identifier 'X' not declared").
//!
//! For these scripts, rewriting is skipped entirely. The vanilla `.gdc` in
//! `RTV.pck` keeps serving the original behavior via VFS fallthrough
//! (no rewritten entry at the same path in the hook pack). Hooks on
//! these scripts' methods won't fire, that's the documented trade-off.
//!
//! # When to add to this list
//!
//! Only when a vanilla script genuinely cannot be runtime-compiled.
//! Performance / timing concerns aren't a reason, for those, fix the
//! template, don't skip the script.

/// Vanilla scripts that crabby ships unmodified.
///
/// Each entry is `(filename, reason)`. The filename matches
/// `ParsedScript::filename` (bare, with `.gd` suffix). The reason is
/// rendered in install diagnostics so users understand why a hook on a
/// skipped script wouldn't fire.
pub const RUNTIME_INCOMPATIBLE_SCRIPTS: &[(&str, &str)] = &[(
    "TreeRenderer.gd",
    "uses `EditorInterface`, an editor-only symbol that doesn't \
         exist at runtime; vanilla bytecode references it but our \
         text-source recompile rejects the unresolved identifier",
)];

/// Whether `filename` is a runtime-incompatible script that should be skipped.
#[must_use]
pub fn is_runtime_incompatible(filename: &str) -> bool {
    RUNTIME_INCOMPATIBLE_SCRIPTS
        .iter()
        .any(|(name, _)| *name == filename)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_known_skip() {
        assert!(is_runtime_incompatible("TreeRenderer.gd"));
    }

    #[test]
    fn rejects_unknown_scripts() {
        assert!(!is_runtime_incompatible("Controller.gd"));
        assert!(!is_runtime_incompatible("treerenderer.gd")); // case-sensitive
        assert!(!is_runtime_incompatible("TreeRenderer")); // requires .gd
    }

    #[test]
    fn every_entry_has_reason() {
        for (name, reason) in RUNTIME_INCOMPATIBLE_SCRIPTS {
            assert!(!name.is_empty(), "empty filename");
            assert!(!reason.is_empty(), "{name}: empty reason");
        }
    }
}
