//! Data-intercept injection.
//!
//! **Disabled.** The `_get(property)` shadowing this module emits
//! doesn't intercept reads of declared `@export` variables, Godot's
//! property resolver hits the storage slot first and never consults
//! `_get`. Confirmed empirically. The registry now mutates live
//! Resource fields directly via `set()`, which works because PCK
//! rewrite gives ownership of the script class binding.
//!
//! The module is kept in place but unused so the format reference
//! survives, and so a future use case (e.g., shadowing computed
//! properties on undeclared names) can re-enable it cheaply.

#![allow(dead_code)]
//!
//! Appends a [`PATCH_DICT_VAR_NAME`](crate::data_intercept::PATCH_DICT_VAR_NAME)
//! field plus a `_get(property)` override so the registry API can shadow
//! `@export var` values on loaded resources at runtime without touching
//! any consumer call site.
//!
//! # Emitted shape
//!
//! ```gdscript
//! # --- Crabby data-intercept injection ---
//! var _rtv_mod_patches: Dictionary = {}
//!
//! func _get(property: StringName):
//!     var key := String(property)
//!     if _rtv_mod_patches.has(key):
//!         return _rtv_mod_patches[key]
//!     return null
//! ```
//!
//! # Why no `_set`
//!
//! Crabby's registry `patch(...)` verb writes the override value into the
//! dict. Reads go through `_get`. Normal game writes to the underlying
//! `@export var` still land on the exported storage, but the dict
//! shadows them on subsequent reads, which is what mods want. A `_set`
//! override would be useful for property-change observers but isn't
//! needed for the basic patch-then-read flow.

use std::fmt::Write as _;

use crate::data_intercept::PATCH_DICT_VAR_NAME;

/// Parameters the data-intercept emitter needs.
pub struct Inputs<'a> {
    /// Indent unit (tab or spaces) matching the script's existing style.
    pub indent: &'a str,
}

/// Emit the data-intercept injection block as a `GDScript` source fragment.
///
/// Returns the block exactly as it should be appended to the script's
/// source after the last vanilla declaration. Starts with a blank line
/// and a comment banner so the injection is clearly set apart in the
/// emitted file.
#[must_use]
pub fn emit(inputs: &Inputs<'_>) -> String {
    let i1 = inputs.indent;

    let mut out = String::new();
    let w = &mut out;

    let _ = writeln!(w, "# --- Crabby data-intercept injection ---");
    let _ = writeln!(w, "var {PATCH_DICT_VAR_NAME}: Dictionary = {{}}");
    let _ = writeln!(w);
    let _ = writeln!(w, "func _get(property: StringName):");
    let _ = writeln!(w, "{i1}var key := String(property)");
    let _ = writeln!(w, "{i1}if {PATCH_DICT_VAR_NAME}.has(key):");
    let _ = writeln!(w, "{i1}{i1}return {PATCH_DICT_VAR_NAME}[key]");
    let _ = writeln!(w, "{i1}return null");

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_patch_dict_and_get_override() {
        let out = emit(&Inputs { indent: "\t" });
        assert!(out.contains("var _rtv_mod_patches: Dictionary = {}"));
        assert!(out.contains("func _get(property: StringName):"));
        assert!(out.contains("if _rtv_mod_patches.has(key):"));
        assert!(out.contains("return _rtv_mod_patches[key]"));
        assert!(out.contains("return null"));
    }

    #[test]
    fn has_comment_banner() {
        let out = emit(&Inputs { indent: "\t" });
        assert!(out.contains("# --- Crabby data-intercept injection ---"));
    }

    #[test]
    fn tab_indent_respected() {
        let out = emit(&Inputs { indent: "\t" });
        // The `var key := ...` line is inside `_get` at one indent level.
        assert!(out.contains("\tvar key := String(property)"));
        // The `return _rtv_mod_patches[key]` is two indents deep.
        assert!(out.contains("\t\treturn _rtv_mod_patches[key]"));
    }

    #[test]
    fn four_space_indent_respected() {
        let out = emit(&Inputs { indent: "    " });
        assert!(out.contains("    var key := String(property)"));
        assert!(out.contains("        return _rtv_mod_patches[key]"));
        assert!(!out.contains('\t'));
    }

    #[test]
    fn emits_no_set_override() {
        // `_set` is intentionally omitted, see module docs.
        let out = emit(&Inputs { indent: "\t" });
        assert!(!out.contains("func _set("));
    }

    #[test]
    fn does_not_leak_hook_dispatch_machinery() {
        // Data-intercept is a pure property-resolution shim; there's no
        // hook dispatch, no _caller, no _wrapper_active, no _dispatch.
        let out = emit(&Inputs { indent: "\t" });
        assert!(!out.contains("_lib"));
        assert!(!out.contains("_caller"));
        assert!(!out.contains("_wrapper_active"));
        assert!(!out.contains("_dispatch"));
    }
}
