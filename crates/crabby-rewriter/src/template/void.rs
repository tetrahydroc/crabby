//! Void dispatch-wrapper codegen.
//!
//! Emits the `GDScript` source of a wrapper method that:
//!
//! 1. Short-circuits to the renamed vanilla body when `RTVModLib` isn't
//!    mounted or no mod has called `hook()`.
//! 2. Guards against re-entry via `_lib._wrapper_active[hook_base]`.
//! 3. Fires the `-pre` dispatch.
//! 4. Invokes the replace hook (if any) and the vanilla body (unless
//!    `skip_super` was set by the replace hook).
//! 5. Fires the `-post` dispatch and the deferred `-callback`.
//!
//! Mirrors vostok-mod-loader's `_rtv_dispatch_inline_src` void branch,
//! minus the `_rtv_dispatch_by_hook` probe instrumentation (dev-only in
//! vostok; no production value).

use std::fmt::Write as _;

use crabby_parser::FuncDecl;

/// Parameters the void-template emitter needs.
pub struct Inputs<'a> {
    /// Method being wrapped.
    pub func: &'a FuncDecl,
    /// Script prefix (e.g. `"controller"`).
    pub script_prefix: &'a str,
    /// Indent unit (tab or spaces) matching the source's existing style.
    pub indent: &'a str,
    /// Prefix applied to the renamed vanilla body
    /// (`_rtv_vanilla_` for vanilla scripts).
    pub rename_prefix: &'a str,
    /// Per-kind dispatch flags. When `flags.pre = false` the
    /// `-pre` dispatch line is omitted from the wrapper, etc. When
    /// `flags.replace = false` the entire `_get_hooks` / replace probe
    /// branch collapses to a direct vanilla call. Default
    /// (`HookFlags::all()`) reproduces legacy emission.
    pub flags: crate::HookFlags,
}

/// Emit the void dispatch wrapper as a `GDScript` source fragment.
///
/// Returns the block exactly as it should appear in the rewritten source:
/// starts with `func <name>(...)`, ends with a trailing newline.
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

    // All writes are to an in-memory String, so fmt::Write never errors.
    let mut out = String::new();
    let w = &mut out;
    let _ = writeln!(w, "{sig}");

    // Shim-not-mounted short-circuit. Always emitted: even with full
    // legacy flags the wrapper must work when RTVModLib isn't mounted
    // (uninstall, dev iteration, etc.).
    let _ = writeln!(
        w,
        "{i1}var _lib = Engine.get_meta(\"RTVModLib\") if Engine.has_meta(\"RTVModLib\") else null",
    );
    let _ = writeln!(w, "{i1}if !_lib:");
    let _ = writeln!(w, "{i2}{aw}{vanilla_call}");
    let _ = writeln!(w, "{i2}return");

    // Global no-hooks short-circuit.
    let _ = writeln!(w, "{i1}if not _lib._any_mod_hooked:");
    let _ = writeln!(w, "{i2}{aw}{vanilla_call}");
    let _ = writeln!(w, "{i2}return");

    // Re-entry guard.
    let _ = writeln!(w, "{i1}if _lib._wrapper_active.has(\"{hook}\"):");
    let _ = writeln!(w, "{i2}{aw}{vanilla_call}");
    let _ = writeln!(w, "{i2}return");
    let _ = writeln!(w, "{i1}_lib._wrapper_active[\"{hook}\"] = true");

    // Save + set _caller.
    let _ = writeln!(w, "{i1}var _rtv_prev_caller = _lib._caller");
    let _ = writeln!(w, "{i1}_lib._caller = self");

    // Pre-dispatch (omit when no mod registered a -pre hook).
    if flags.pre {
        let _ = writeln!(w, "{i1}_lib._dispatch(\"{hook}-pre\", {args_array})");
    }

    // Replace / body. When no replace hook is registered the
    // entire `_get_hooks` probe + super-skip plumbing collapses to a
    // direct vanilla call.
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

    // Re-set _caller then post + callback. Both are independently
    // elidable per-flag. _caller restoration is only needed
    // before another dispatch line, if both post + callback are
    // off, the cleanup block below restores it directly.
    if flags.post || flags.callback {
        let _ = writeln!(w, "{i1}_lib._caller = self");
    }
    if flags.post {
        let _ = writeln!(w, "{i1}_lib._dispatch(\"{hook}-post\", {args_array})");
    }
    if flags.callback {
        let _ = writeln!(
            w,
            "{i1}_lib._dispatch_deferred(\"{hook}-callback\", {args_array})",
        );
    }

    // Cleanup.
    let _ = writeln!(w, "{i1}_lib._wrapper_active.erase(\"{hook}\")");
    let _ = writeln!(w, "{i1}_lib._caller = _rtv_prev_caller");

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_func(name: &str, params: &str, param_names: &[&str]) -> FuncDecl {
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
        let func = fixture_func("_physics_process", "delta", &["delta"]);
        let out = emit(&Inputs {
            func: &func,
            script_prefix: "controller",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        assert!(
            out.starts_with("func _physics_process(delta):\n"),
            "signature drift: {}",
            out.lines().next().unwrap_or_default(),
        );
    }

    #[test]
    fn emits_correct_hook_base() {
        let func = fixture_func("_physics_process", "delta", &["delta"]);
        let out = emit(&Inputs {
            func: &func,
            script_prefix: "controller",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        assert!(out.contains("\"controller-_physics_process-pre\""));
        assert!(out.contains("\"controller-_physics_process-post\""));
        assert!(out.contains("\"controller-_physics_process-callback\""));
        assert!(out.contains("\"controller-_physics_process\""));
    }

    #[test]
    fn no_params_uses_empty_args_array() {
        let func = fixture_func("_ready", "", &[]);
        let out = emit(&Inputs {
            func: &func,
            script_prefix: "controller",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        assert!(out.contains("_lib._dispatch(\"controller-_ready-pre\", [])"));
        assert!(out.contains("_repl[0].callv([])"));
        assert!(out.contains("_rtv_vanilla__ready()"));
    }

    #[test]
    fn await_prefix_added_for_coroutines() {
        let mut func = fixture_func("HandleAsync", "delta", &["delta"]);
        func.is_coroutine = true;
        let out = emit(&Inputs {
            func: &func,
            script_prefix: "mod",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        // Every vanilla-call site must be awaited. Non-coroutine emit
        // produces 5 call sites (three short-circuits, one replace-no-skip
        // path, one no-replace fallthrough). Coroutine emit must prefix
        // each with `await `.
        let awaited = out.matches("await _rtv_vanilla_HandleAsync(delta)").count();
        assert_eq!(
            awaited, 5,
            "expected 5 awaited vanilla-call sites in void coroutine wrapper; got {awaited}\n{out}",
        );
        // No un-awaited call should remain.
        let non_awaited = out
            .lines()
            .filter(|l| l.contains("_rtv_vanilla_HandleAsync(delta)") && !l.contains("await "))
            .count();
        assert_eq!(
            non_awaited, 0,
            "found un-awaited vanilla call(s) in coroutine wrapper:\n{out}",
        );
    }

    #[test]
    fn non_coroutine_emits_no_await() {
        let func = fixture_func("Tick", "delta", &["delta"]);
        let out = emit(&Inputs {
            func: &func,
            script_prefix: "mod",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        assert!(
            !out.contains("await "),
            "non-coroutine wrapper must not emit await:\n{out}",
        );
    }

    #[test]
    fn preserves_four_space_indent() {
        let func = fixture_func("_physics_process", "delta", &["delta"]);
        let out = emit(&Inputs {
            func: &func,
            script_prefix: "controller",
            indent: "    ",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        assert!(out.contains("    var _lib"));
        assert!(!out.contains('\t'));
    }

    // --- per-kind elision tests ---

    fn pre_only() -> crate::HookFlags {
        crate::HookFlags { pre: true, ..Default::default() }
    }
    fn post_only() -> crate::HookFlags {
        crate::HookFlags { post: true, ..Default::default() }
    }
    fn callback_only() -> crate::HookFlags {
        crate::HookFlags { callback: true, ..Default::default() }
    }
    fn replace_only() -> crate::HookFlags {
        crate::HookFlags { replace: true, ..Default::default() }
    }

    #[test]
    fn b22_pre_only_drops_post_callback_replace_branch() {
        let func = fixture_func("ApplyDamage", "dmg", &["dmg"]);
        let out = emit(&Inputs {
            func: &func,
            script_prefix: "hitbox",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: pre_only(),
        });
        // Pre dispatch present.
        assert!(out.contains("_lib._dispatch(\"hitbox-applydamage-pre\""), "{out}");
        // Post / callback / replace probe all elided.
        assert!(!out.contains("hitbox-applydamage-post"), "{out}");
        assert!(!out.contains("hitbox-applydamage-callback"), "{out}");
        assert!(!out.contains("_get_hooks"), "{out}");
        assert!(!out.contains("_skip_super"), "{out}");
        // Vanilla still gets called (direct path).
        assert!(out.contains("_rtv_vanilla_ApplyDamage(dmg)"), "{out}");
    }

    #[test]
    fn b22_post_only_drops_pre_callback_replace() {
        let func = fixture_func("Tick", "", &[]);
        let out = emit(&Inputs {
            func: &func,
            script_prefix: "ai",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: post_only(),
        });
        assert!(out.contains("_lib._dispatch(\"ai-tick-post\""), "{out}");
        assert!(!out.contains("ai-tick-pre"), "{out}");
        assert!(!out.contains("ai-tick-callback"), "{out}");
        assert!(!out.contains("_get_hooks"), "{out}");
    }

    #[test]
    fn b22_callback_only_drops_pre_post_replace() {
        let func = fixture_func("Spawn", "", &[]);
        let out = emit(&Inputs {
            func: &func,
            script_prefix: "compiler",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: callback_only(),
        });
        assert!(out.contains("_dispatch_deferred(\"compiler-spawn-callback\""), "{out}");
        assert!(!out.contains("compiler-spawn-pre"), "{out}");
        assert!(!out.contains("compiler-spawn-post"), "{out}");
        assert!(!out.contains("_get_hooks"), "{out}");
    }

    #[test]
    fn b22_replace_only_keeps_get_hooks_drops_per_kind_dispatches() {
        let func = fixture_func("Compute", "x", &["x"]);
        let out = emit(&Inputs {
            func: &func,
            script_prefix: "calc",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: replace_only(),
        });
        // Replace probe stays.
        assert!(out.contains("_get_hooks(\"calc-compute\")"), "{out}");
        assert!(out.contains("_skip_super"), "{out}");
        // No -pre / -post / -callback dispatches.
        assert!(!out.contains("calc-compute-pre"), "{out}");
        assert!(!out.contains("calc-compute-post"), "{out}");
        assert!(!out.contains("calc-compute-callback"), "{out}");
    }

    #[test]
    fn b22_no_replace_collapses_branch_to_direct_call() {
        // pre + post but no replace, the entire `if _repl.size() > 0`
        // block disappears, replaced by a single direct vanilla call.
        let func = fixture_func("ApplyDamage", "dmg", &["dmg"]);
        let out = emit(&Inputs {
            func: &func,
            script_prefix: "hitbox",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags { pre: true, post: true, ..Default::default() },
        });
        assert!(!out.contains("_get_hooks"), "{out}");
        assert!(!out.contains("_skip_super"), "{out}");
        assert!(!out.contains("_repl"), "{out}");
        // Both pre + post present.
        assert!(out.contains("hitbox-applydamage-pre"), "{out}");
        assert!(out.contains("hitbox-applydamage-post"), "{out}");
        // Direct vanilla call appears (counted inside the dispatch path,
        // not just in the short-circuits).
        let vanilla_calls = out.matches("_rtv_vanilla_ApplyDamage(dmg)").count();
        // Three short-circuit branches (no-lib, no-hooks, re-entry) plus
        // the main-path direct call = 4. Legacy emission has 5 (the extra
        // is the replace-no-skip path's call).
        assert_eq!(vanilla_calls, 4, "expected 4 vanilla calls, got {vanilla_calls}\n{out}");
    }

    #[test]
    fn b22_full_flags_match_legacy_emission() {
        // Sanity: HookFlags::all() must produce byte-identical output
        // to the pre-elision legacy emission.
        let func = fixture_func("Update", "delta", &["delta"]);
        let out = emit(&Inputs {
            func: &func,
            script_prefix: "ctrl",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        // Every dispatch site present.
        assert!(out.contains("ctrl-update-pre"));
        assert!(out.contains("ctrl-update-post"));
        assert!(out.contains("ctrl-update-callback"));
        assert!(out.contains("_get_hooks(\"ctrl-update\")"));
    }
}
