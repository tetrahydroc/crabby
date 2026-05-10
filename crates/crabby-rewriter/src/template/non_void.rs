//! Non-void dispatch-wrapper codegen.
//!
//! Parallels the void template but threads a `_result` value through every
//! branch and returns it. Short-circuit paths (shim-missing,
//! `!_any_mod_hooked`, re-entry guard) return the renamed vanilla body's
//! value directly without touching `_result`.
//!
//! Mirrors vostok-mod-loader's `_rtv_dispatch_inline_src` non-void branch,
//! minus the `_rtv_dispatch_by_hook` probe instrumentation.

use std::fmt::Write as _;

use crabby_parser::FuncDecl;

use super::void::Inputs;

/// Emit the non-void dispatch wrapper as a `GDScript` source fragment.
#[must_use]
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
    let sig = signature(func);

    let mut out = String::new();
    let w = &mut out;
    let _ = writeln!(w, "{sig}");

    // Shim-not-mounted short-circuit.
    let _ = writeln!(
        w,
        "{i1}var _lib = Engine.get_meta(\"RTVModLib\") if Engine.has_meta(\"RTVModLib\") else null",
    );
    let _ = writeln!(w, "{i1}if !_lib:");
    let _ = writeln!(w, "{i2}return {aw}{vanilla_call}");

    // Global no-hooks short-circuit.
    let _ = writeln!(w, "{i1}if not _lib._any_mod_hooked:");
    let _ = writeln!(w, "{i2}return {aw}{vanilla_call}");

    // Re-entry guard.
    let _ = writeln!(w, "{i1}if _lib._wrapper_active.has(\"{hook}\"):");
    let _ = writeln!(w, "{i2}return {aw}{vanilla_call}");
    let _ = writeln!(w, "{i1}_lib._wrapper_active[\"{hook}\"] = true");

    // Save + set _caller.
    let _ = writeln!(w, "{i1}var _rtv_prev_caller = _lib._caller");
    let _ = writeln!(w, "{i1}_lib._caller = self");

    // Pre-dispatch (omit when no -pre hook is registered).
    if flags.pre {
        let _ = writeln!(w, "{i1}_lib._dispatch(\"{hook}-pre\", {args_array})");
    }

    // Result computation. With no replace hook the probe +
    // super-skip plumbing collapses to a direct `_result = vanilla()`.
    let _ = writeln!(w, "{i1}var _result");
    if flags.replace {
        let _ = writeln!(w, "{i1}var _repl = _lib._get_hooks(\"{hook}\")");
        let _ = writeln!(w, "{i1}if _repl.size() > 0:");
        let _ = writeln!(w, "{i2}var _prev_skip = _lib._skip_super");
        let _ = writeln!(w, "{i2}_lib._skip_super = false");
        let _ = writeln!(w, "{i2}var _replret = _repl[0].callv({args_array})");
        let _ = writeln!(w, "{i2}var _did_skip = _lib._skip_super");
        let _ = writeln!(w, "{i2}_lib._skip_super = _prev_skip");
        let _ = writeln!(w, "{i2}if _did_skip:");
        let _ = writeln!(w, "{i3}_result = _replret");
        let _ = writeln!(w, "{i2}else:");
        let _ = writeln!(w, "{i3}_result = {aw}{vanilla_call}");
        let _ = writeln!(w, "{i1}else:");
        let _ = writeln!(w, "{i2}_result = {aw}{vanilla_call}");
    } else {
        let _ = writeln!(w, "{i1}_result = {aw}{vanilla_call}");
    }

    // Re-set _caller then post + callback. Non-void post-dispatch goes
    // through `_dispatch_post`, which lets each callback mutate the
    // running `_result` by returning non-null. See shim's
    // `_dispatch_post` for the contract (legacy 2-arg callbacks still
    // work via arity detection, with a one-shot deprecation warning).
    if flags.post || flags.callback {
        let _ = writeln!(w, "{i1}_lib._caller = self");
    }
    if flags.post {
        let _ = writeln!(
            w,
            "{i1}_result = _lib._dispatch_post(\"{hook}-post\", {args_array}, _result)",
        );
    }
    if flags.callback {
        let _ = writeln!(
            w,
            "{i1}_lib._dispatch_deferred(\"{hook}-callback\", {args_array})",
        );
    }

    // Cleanup + return.
    let _ = writeln!(w, "{i1}_lib._wrapper_active.erase(\"{hook}\")");
    let _ = writeln!(w, "{i1}_lib._caller = _rtv_prev_caller");
    let _ = writeln!(w, "{i1}return _result");

    out
}

fn signature(func: &FuncDecl) -> String {
    let mut sig = if func.params.is_empty() {
        format!("func {}()", func.name)
    } else {
        format!("func {}({})", func.name, func.params)
    };
    if let Some(ret) = func.return_type.as_deref() {
        sig.push_str(" -> ");
        sig.push_str(ret);
    }
    sig.push(':');
    sig
}

#[cfg(test)]
mod tests {
    use super::*;

    fn func(name: &str, params: &str, param_names: &[&str], ret: Option<&str>) -> FuncDecl {
        FuncDecl {
            name: name.to_owned(),
            params: params.to_owned(),
            param_names: param_names.iter().map(|s| (*s).to_owned()).collect(),
            line_number: 1,
            is_static: false,
            return_type: ret.map(ToOwned::to_owned),
            is_coroutine: false,
            has_return_value: true,
        }
    }

    #[test]
    fn signature_preserves_return_type() {
        let f = func("add", "a: int, b: int", &["a", "b"], Some("int"));
        let out = emit(&Inputs {
            func: &f,
            script_prefix: "calc",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        assert!(
            out.starts_with("func add(a: int, b: int) -> int:\n"),
            "got: {out}"
        );
    }

    #[test]
    fn signature_without_return_type_omits_arrow() {
        let f = func("guess", "x", &["x"], None);
        let out = emit(&Inputs {
            func: &f,
            script_prefix: "calc",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        assert!(out.starts_with("func guess(x):\n"), "got: {out}");
        assert!(!out.contains(" -> "));
    }

    #[test]
    fn body_returns_result_through_all_branches() {
        let f = func("calc", "x", &["x"], Some("int"));
        let out = emit(&Inputs {
            func: &f,
            script_prefix: "calc",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        // Short-circuit branches: `return <vanilla>`, not `return _result`.
        assert!(out.contains("if !_lib:\n\t\treturn _rtv_vanilla_calc(x)"));
        assert!(out.contains("if not _lib._any_mod_hooked:\n\t\treturn _rtv_vanilla_calc(x)"));
        // Main path: computes _result, then returns it at the very end.
        assert!(out.contains("var _result\n"));
        assert!(out.ends_with("\treturn _result\n"));
    }

    #[test]
    fn replace_path_threads_skip_super() {
        let f = func("calc", "x", &["x"], Some("int"));
        let out = emit(&Inputs {
            func: &f,
            script_prefix: "calc",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        assert!(out.contains("var _replret = _repl[0].callv([x])"));
        assert!(out.contains("if _did_skip:\n\t\t\t_result = _replret"));
        assert!(out.contains("else:\n\t\t\t_result = _rtv_vanilla_calc(x)"));
    }

    #[test]
    fn await_prefix_added_for_coroutine() {
        let mut f = func("fetch", "", &[], Some("int"));
        f.is_coroutine = true;
        let out = emit(&Inputs {
            func: &f,
            script_prefix: "net",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        // Non-void coroutine wrapper: 3 short-circuit returns + 2 `_result`
        // assignments = 5 vanilla-call sites, every one must be awaited.
        let awaited = out.matches("await _rtv_vanilla_fetch()").count();
        assert_eq!(
            awaited, 5,
            "expected 5 awaited vanilla-call sites in non-void coroutine wrapper; got {awaited}\n{out}",
        );
        // No un-awaited call should remain.
        let non_awaited = out
            .lines()
            .filter(|l| l.contains("_rtv_vanilla_fetch()") && !l.contains("await "))
            .count();
        assert_eq!(
            non_awaited, 0,
            "found un-awaited vanilla call(s) in coroutine wrapper:\n{out}",
        );
    }

    #[test]
    fn non_coroutine_emits_no_await() {
        let f = func("calc", "x", &["x"], Some("int"));
        let out = emit(&Inputs {
            func: &f,
            script_prefix: "calc",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        assert!(
            !out.contains("await "),
            "non-coroutine non-void wrapper must not emit await:\n{out}",
        );
    }

    #[test]
    fn post_dispatch_routes_through_dispatch_post_for_chained_mutation() {
        // The post-dispatch line must use `_dispatch_post` (chained
        // mutator) rather than plain `_dispatch`, AND must thread the
        // running `_result` through it so each callback can transform
        // the return value. See shim's `_dispatch_post` for the contract.
        let f = func("calc", "x", &["x"], Some("int"));
        let out = emit(&Inputs {
            func: &f,
            script_prefix: "calc",
            indent: "\t",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        assert!(
            out.contains("_result = _lib._dispatch_post(\"calc-calc-post\", [x], _result)"),
            "non-void post-dispatch must thread _result through _dispatch_post:\n{out}",
        );
        // Plain `_dispatch("...-post", ...)` would silently drop returns;
        // make sure no stray legacy emission slipped in.
        assert!(
            !out.contains("_lib._dispatch(\"calc-calc-post\""),
            "non-void wrapper must not emit the legacy plain _dispatch for post:\n{out}",
        );
    }

    #[test]
    fn four_space_indent_respected() {
        let f = func("calc", "x", &["x"], Some("int"));
        let out = emit(&Inputs {
            func: &f,
            script_prefix: "calc",
            indent: "    ",
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        });
        assert!(!out.contains('\t'));
        assert!(out.contains("    var _lib"));
    }
}
