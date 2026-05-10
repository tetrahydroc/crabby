//! Fast dispatch-wrapper codegen.
//!
//! Used for scripts where per-call dispatch overhead is visible in profile:
//! per-frame effects (`MuzzleFlash`), per-shot projectiles (`Hit`),
//! short-lived instances (`Mine`). Vostok-mod-loader skips these scripts
//! entirely for dispatch overhead reasons; crabby keeps them hookable by
//! emitting a lighter wrapper instead.
//!
//! # How it differs from the standard void template
//!
//! - **Merged `!_lib` + `!_any_mod_hooked` short-circuit**: one early
//!   return handles both "no shim mounted" and "no mod has hooked anything"
//!   cases, routing to the renamed vanilla body with no
//!   dispatch overhead.
//! - **No re-entry guard** (`_wrapper_active`): fast-templated methods are
//!   contracted to be short-lived and non-recursive, so the dict ops and
//!   active-set bookkeeping are skipped.
//! - **No `_caller` save/restore**: the template sets `_lib._caller`
//!   before pre-dispatch but does not save/restore a prior value. Callers
//!   downstream of a fast wrapper cannot rely on `_caller` surviving a
//!   nested fast -> standard wrapper chain.
//!
//! # What's unchanged
//!
//! Hook-name format (`<script>-<method>[-pre|-post|-callback]`), replace
//! hook + `skip_super` semantics, deferred `-callback` dispatch, `-pre`
//! and `-post` firing order. Mods see the same API whether their target
//! is fast- or standard-templated.
//!
//! # Tradeoff
//!
//! Without the re-entry guard, a replace hook that directly re-enters a
//! fast-templated method on the same instance would recurse infinitely.
//! That's vanishingly rare for the targeted script list (one-shot effects
//! don't call themselves), and documented here as a known constraint.
//!
//! Fast-templated methods are always void, they're called by Godot for
//! their side effects and never have a return value a replace hook could
//! meaningfully take over. The emitter asserts this in debug builds via
//! the caller's template-selection layer; this module itself doesn't
//! enforce it.

use std::fmt::Write as _;

use super::void::Inputs;

/// Emit the fast dispatch wrapper as a `GDScript` source fragment.
pub fn emit(inputs: &Inputs<'_>) -> String {
    let Inputs {
        func,
        script_prefix,
        indent,
        rename_prefix,
        flags,
    } = *inputs;

    let i1 = indent;
    let i2 = format!("{indent}{indent}");
    let i3 = format!("{indent}{indent}{indent}");

    let hook = crate::hook_name::hook_base(script_prefix, &func.name);
    let param_names = func.param_names.join(", ");
    let args_array = if func.param_names.is_empty() {
        "[]".to_owned()
    } else {
        format!("[{param_names}]")
    };
    let vanilla_call = format!("{rename_prefix}{}({param_names})", func.name);
    let aw = if func.is_coroutine { "await " } else { "" };

    let sig = if func.params.is_empty() {
        format!("func {}():", func.name)
    } else {
        format!("func {}({}):", func.name, func.params)
    };

    let mut out = String::new();
    let w = &mut out;
    let _ = writeln!(w, "{sig}");

    // Merged short-circuit: either no shim mounted OR no mod has hooked
    // anything -> straight to the renamed vanilla body.
    let _ = writeln!(
        w,
        "{i1}var _lib = Engine.get_meta(\"RTVModLib\") if Engine.has_meta(\"RTVModLib\") else null",
    );
    let _ = writeln!(w, "{i1}if not _lib or not _lib._any_mod_hooked:");
    let _ = writeln!(w, "{i2}{aw}{vanilla_call}");
    let _ = writeln!(w, "{i2}return");

    // Hooked path. Set _caller without save/restore, fast contract.
    let _ = writeln!(w, "{i1}_lib._caller = self");
    if flags.pre {
        let _ = writeln!(w, "{i1}_lib._dispatch(\"{hook}-pre\", {args_array})");
    }

    // Replace + vanilla body. With no replace hook the probe
    // collapses to a direct vanilla call.
    if flags.replace {
        let _ = writeln!(w, "{i1}var _repl = _lib._get_hooks(\"{hook}\")");
        let _ = writeln!(w, "{i1}if _repl.size() > 0:");
        let _ = writeln!(w, "{i2}var _prev_skip = _lib._skip_super");
        let _ = writeln!(w, "{i2}_lib._skip_super = false");
        let _ = writeln!(w, "{i2}_repl[0].callv({args_array})");
        let _ = writeln!(w, "{i2}var _did_skip = _lib._skip_super");
        let _ = writeln!(w, "{i2}_lib._skip_super = _prev_skip");
        let _ = writeln!(w, "{i2}if !_did_skip:");
        let _ = writeln!(w, "{i3}{aw}{vanilla_call}");
        let _ = writeln!(w, "{i1}else:");
        let _ = writeln!(w, "{i2}{aw}{vanilla_call}");
    } else {
        let _ = writeln!(w, "{i1}{aw}{vanilla_call}");
    }

    // Post + callback (each independently elidable).
    if flags.post {
        let _ = writeln!(w, "{i1}_lib._dispatch(\"{hook}-post\", {args_array})");
    }
    if flags.callback {
        let _ = writeln!(
            w,
            "{i1}_lib._dispatch_deferred(\"{hook}-callback\", {args_array})",
        );
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabby_parser::FuncDecl;

    fn fixture(name: &str, params: &str, param_names: &[&str]) -> FuncDecl {
        FuncDecl {
            name: name.to_owned(),
            params: params.to_owned(),
            param_names: param_names.iter().map(|s| (*s).to_owned()).collect(),
            line_number: 1,
            is_static: false,
            return_type: None,
            is_coroutine: false,
            has_return_value: false,
        }
    }

    #[test]
    fn emits_signature_preserving_params() {
        let f = fixture("update", "delta: float", &["delta"]);
        let out = emit(&Inputs {
            func: &f,
            script_prefix: "muzzleflash",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        assert!(
            out.starts_with("func update(delta: float):\n"),
            "got: {out}"
        );
    }

    #[test]
    fn short_circuit_handles_both_no_lib_and_no_hooks() {
        let f = fixture("update", "delta", &["delta"]);
        let out = emit(&Inputs {
            func: &f,
            script_prefix: "muzzleflash",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        // One combined early-return, not two.
        assert!(out.contains("if not _lib or not _lib._any_mod_hooked:"));
        // Separate checks in the standard template would look different;
        // confirm none accidentally remained.
        assert!(!out.contains("if !_lib:"));
        assert!(!out.contains("if not _lib._any_mod_hooked:\n\t\t_rtv_vanilla"));
    }

    #[test]
    fn no_wrapper_active_re_entry_guard() {
        let f = fixture("update", "delta", &["delta"]);
        let out = emit(&Inputs {
            func: &f,
            script_prefix: "muzzleflash",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        assert!(!out.contains("_wrapper_active"));
    }

    #[test]
    fn no_caller_save_restore() {
        let f = fixture("update", "delta", &["delta"]);
        let out = emit(&Inputs {
            func: &f,
            script_prefix: "muzzleflash",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        // _caller is set (for pre/post hooks) but never saved or restored.
        assert!(out.contains("_lib._caller = self"));
        assert!(!out.contains("_rtv_prev_caller"));
    }

    #[test]
    fn dispatch_names_match_standard_convention() {
        let f = fixture("update", "delta", &["delta"]);
        let out = emit(&Inputs {
            func: &f,
            script_prefix: "muzzleflash",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        assert!(out.contains("\"muzzleflash-update-pre\""));
        assert!(out.contains("\"muzzleflash-update-post\""));
        assert!(out.contains("\"muzzleflash-update-callback\""));
        assert!(out.contains("\"muzzleflash-update\""));
    }

    #[test]
    fn replace_and_skip_super_path_preserved() {
        let f = fixture("update", "delta", &["delta"]);
        let out = emit(&Inputs {
            func: &f,
            script_prefix: "muzzleflash",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        assert!(out.contains("_repl[0].callv([delta])"));
        assert!(out.contains("if !_did_skip:"));
    }

    #[test]
    fn await_prefix_added_for_coroutines() {
        let mut f = fixture("update", "delta", &["delta"]);
        f.is_coroutine = true;
        let out = emit(&Inputs {
            func: &f,
            script_prefix: "muzzleflash",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        // All three vanilla-call sites get the await prefix.
        let awaits = out.matches("await _rtv_vanilla_update(delta)").count();
        assert_eq!(awaits, 3, "expected 3 await sites, got {awaits} in:\n{out}");
        // No un-awaited call should remain.
        let non_awaited = out
            .lines()
            .filter(|l| l.contains("_rtv_vanilla_update(delta)") && !l.contains("await "))
            .count();
        assert_eq!(
            non_awaited, 0,
            "found un-awaited vanilla call(s) in coroutine wrapper:\n{out}",
        );
    }

    #[test]
    fn non_coroutine_emits_no_await() {
        let f = fixture("update", "delta", &["delta"]);
        let out = emit(&Inputs {
            func: &f,
            script_prefix: "muzzleflash",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        assert!(
            !out.contains("await "),
            "non-coroutine fast wrapper must not emit await:\n{out}",
        );
    }

    #[test]
    fn four_space_indent_respected() {
        let f = fixture("update", "delta", &["delta"]);
        let out = emit(&Inputs {
            func: &f,
            script_prefix: "muzzleflash",
            indent: "    ",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        assert!(!out.contains('\t'));
        assert!(out.contains("    var _lib"));
    }
}
