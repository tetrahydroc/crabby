//! Additive dispatch-wrapper codegen.
//!
//! Used for `Resource`-subclass scripts whose method names are embedded
//! in persisted save files. The vanilla body stays put under its
//! original name so `ResourceSaver` / `ResourceLoader` round-trips stay
//! compatible; a new method is emitted under the
//! [`ADDITIVE_HOOK_PREFIX`](crate::resource_serialized::ADDITIVE_HOOK_PREFIX)
//! alongside it that handles dispatch and eventually calls `self.<name>()`.
//!
//! Consumers elsewhere in the rewritten corpus have their call sites
//! redirected to the prefixed wrapper by [`crate::call_rewriter`]. The
//! vanilla body is reachable only via:
//!
//! - `ResourceSaver` / `ResourceLoader` serializing+deserializing saves
//!   (by design, that's the compatibility being protected)
//! - Intra-script calls inside the resource-serialized script itself
//!   (consumer rewriting deliberately skips the script's own file)
//! - `self.<name>(...)` inside the wrapper itself (the one call the
//!   wrapper emits)
//!
//! # Void vs non-void
//!
//! Shape mirrors the [void](super::void) and [`non_void`](super::non_void)
//! templates, minus the rename: the "vanilla call" site is the original
//! method name on `self`, not a renamed prefix. Template body structure
//! (short-circuits, re-entry guard, `_caller` save/restore, dispatch
//! order) is identical to the standard templates.
//!
//! # Hook name
//!
//! Hook base is `"<script>-<method>"` using the original method name,
//! identical to what the standard template emits. Mods hooking a
//! resource-serialized method use the same hook base they'd use for any
//! other method; the additive template is an implementation detail they
//! shouldn't have to know about.

use std::fmt::Write as _;

use crabby_parser::FuncDecl;

use crate::resource_serialized::ADDITIVE_HOOK_PREFIX;

use super::void::Inputs;

/// Emit the additive dispatch wrapper as a `GDScript` source fragment.
///
/// Picks the void or non-void body shape based on
/// [`FuncDecl::has_return_value`](crabby_parser::FuncDecl::has_return_value).
/// Resource-serialized scripts overwhelmingly use void methods (setters,
/// save-applicators); the non-void branch exists for completeness so
/// things like `Validator.validate() -> bool` still get hooked.
pub fn emit(inputs: &Inputs<'_>) -> String {
    if inputs.func.has_return_value {
        emit_non_void(inputs)
    } else {
        emit_void(inputs)
    }
}

fn emit_void(inputs: &Inputs<'_>) -> String {
    let Inputs {
        func,
        script_prefix,
        indent,
        flags,
        ..
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
    // Call the original method on self, its name is unchanged.
    let vanilla_call = format!("self.{}({param_names})", func.name);
    let aw = if func.is_coroutine { "await " } else { "" };

    let sig = build_signature(func, ADDITIVE_HOOK_PREFIX);

    let mut out = String::new();
    let w = &mut out;
    let _ = writeln!(w, "{sig}");

    let _ = writeln!(
        w,
        "{i1}var _lib = Engine.get_meta(\"RTVModLib\") if Engine.has_meta(\"RTVModLib\") else null",
    );
    let _ = writeln!(w, "{i1}if !_lib:");
    let _ = writeln!(w, "{i2}{aw}{vanilla_call}");
    let _ = writeln!(w, "{i2}return");

    let _ = writeln!(w, "{i1}if not _lib._any_mod_hooked:");
    let _ = writeln!(w, "{i2}{aw}{vanilla_call}");
    let _ = writeln!(w, "{i2}return");

    let _ = writeln!(w, "{i1}if _lib._wrapper_active.has(\"{hook}\"):");
    let _ = writeln!(w, "{i2}{aw}{vanilla_call}");
    let _ = writeln!(w, "{i2}return");
    let _ = writeln!(w, "{i1}_lib._wrapper_active[\"{hook}\"] = true");

    let _ = writeln!(w, "{i1}var _rtv_prev_caller = _lib._caller");
    let _ = writeln!(w, "{i1}_lib._caller = self");

    if flags.pre {
        let _ = writeln!(w, "{i1}_lib._dispatch(\"{hook}-pre\", {args_array})");
    }

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

    let _ = writeln!(w, "{i1}_lib._wrapper_active.erase(\"{hook}\")");
    let _ = writeln!(w, "{i1}_lib._caller = _rtv_prev_caller");

    out
}

fn emit_non_void(inputs: &Inputs<'_>) -> String {
    let Inputs {
        func,
        script_prefix,
        indent,
        flags,
        ..
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
    let vanilla_call = format!("self.{}({param_names})", func.name);
    let aw = if func.is_coroutine { "await " } else { "" };

    let sig = build_signature(func, ADDITIVE_HOOK_PREFIX);

    let mut out = String::new();
    let w = &mut out;
    let _ = writeln!(w, "{sig}");

    let _ = writeln!(
        w,
        "{i1}var _lib = Engine.get_meta(\"RTVModLib\") if Engine.has_meta(\"RTVModLib\") else null",
    );
    let _ = writeln!(w, "{i1}if !_lib:");
    let _ = writeln!(w, "{i2}return {aw}{vanilla_call}");

    let _ = writeln!(w, "{i1}if not _lib._any_mod_hooked:");
    let _ = writeln!(w, "{i2}return {aw}{vanilla_call}");

    let _ = writeln!(w, "{i1}if _lib._wrapper_active.has(\"{hook}\"):");
    let _ = writeln!(w, "{i2}return {aw}{vanilla_call}");
    let _ = writeln!(w, "{i1}_lib._wrapper_active[\"{hook}\"] = true");

    let _ = writeln!(w, "{i1}var _rtv_prev_caller = _lib._caller");
    let _ = writeln!(w, "{i1}_lib._caller = self");

    if flags.pre {
        let _ = writeln!(w, "{i1}_lib._dispatch(\"{hook}-pre\", {args_array})");
    }

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

    // Non-void post-dispatch: see `non_void::emit` and the shim's
    // `_dispatch_post` for the chained-mutator contract.
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

    let _ = writeln!(w, "{i1}_lib._wrapper_active.erase(\"{hook}\")");
    let _ = writeln!(w, "{i1}_lib._caller = _rtv_prev_caller");
    let _ = writeln!(w, "{i1}return _result");

    out
}

/// Signature for the additive wrapper: `func <prefix><name>(...) [-> T]:`.
fn build_signature(func: &FuncDecl, hook_prefix: &str) -> String {
    let mut sig = if func.params.is_empty() {
        format!("func {hook_prefix}{}()", func.name)
    } else {
        format!("func {hook_prefix}{}({})", func.name, func.params)
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

    fn inputs<'a>(func: &'a FuncDecl, script_prefix: &'a str) -> Inputs<'a> {
        Inputs {
            func,
            script_prefix,
            indent: "\t",
            // rename_prefix is unused for additive (the vanilla body
            // isn't renamed), but the field stays stable at the Inputs
            // struct level so callers don't have to special-case.
            rename_prefix: "_rtv_vanilla_",
            flags: crate::HookFlags::all(),
        }
    }

    #[test]
    fn void_signature_uses_additive_prefix_not_rename_prefix() {
        let f = fixture("save_data", "dict: Dictionary", &["dict"]);
        let out = emit(&inputs(&f, "worldsave"));
        assert!(
            out.starts_with("func _rtv_hooked_save_data(dict: Dictionary):\n"),
            "got:\n{out}"
        );
        // Body invokes the ORIGINAL, unrenamed method on self.
        assert!(out.contains("self.save_data(dict)"));
        assert!(!out.contains("_rtv_vanilla_"));
    }

    #[test]
    fn hook_base_uses_original_method_name() {
        let f = fixture("save_data", "dict: Dictionary", &["dict"]);
        let out = emit(&inputs(&f, "worldsave"));
        // Mods hook `worldsave-save_data-pre`, NOT `worldsave-_rtv_hooked_save_data-pre`.
        assert!(out.contains("\"worldsave-save_data-pre\""));
        assert!(!out.contains("_rtv_hooked_save_data-pre"));
    }

    #[test]
    fn void_body_has_no_result_threading() {
        let f = fixture("save_data", "dict", &["dict"]);
        let out = emit(&inputs(&f, "worldsave"));
        assert!(!out.contains("var _result"));
        assert!(!out.contains("return _result"));
    }

    #[test]
    fn non_void_body_threads_result_and_returns() {
        let mut f = fixture("validate", "", &[]);
        f.has_return_value = true;
        f.return_type = Some("bool".into());
        let out = emit(&inputs(&f, "validator"));
        assert!(out.contains("func _rtv_hooked_validate() -> bool:"));
        assert!(out.contains("var _result"));
        assert!(out.ends_with("return _result\n"));
        // Vanilla call is to the original name on self.
        assert!(out.contains("_result = self.validate()"));
    }

    #[test]
    fn non_void_short_circuits_return_self_call() {
        let mut f = fixture("validate", "", &[]);
        f.has_return_value = true;
        f.return_type = Some("bool".into());
        let out = emit(&inputs(&f, "validator"));
        assert!(out.contains("if !_lib:\n\t\treturn self.validate()"));
        assert!(out.contains("if not _lib._any_mod_hooked:\n\t\treturn self.validate()"));
    }

    #[test]
    fn non_void_post_routes_through_dispatch_post() {
        // Additive non-void variant must emit `_dispatch_post` so post
        // hooks can mutate the running `_result`. Mirrors the
        // `non_void::emit` test; same contract documented on the shim.
        let mut f = fixture("validate", "", &[]);
        f.has_return_value = true;
        f.return_type = Some("bool".into());
        let out = emit(&inputs(&f, "validator"));
        assert!(
            out.contains("_result = _lib._dispatch_post(\"validator-validate-post\", [], _result)"),
            "additive non-void wrapper must thread _result through _dispatch_post:\n{out}",
        );
        assert!(
            !out.contains("_lib._dispatch(\"validator-validate-post\""),
            "additive non-void wrapper must not emit the legacy plain _dispatch for post:\n{out}",
        );
    }

    #[test]
    fn void_post_keeps_plain_dispatch() {
        // The void variant has no _result to mutate, so post-dispatch
        // stays on the simpler `_dispatch` path. Pin this so a future
        // refactor doesn't accidentally route void posts through the
        // chained-mutator path.
        let f = fixture("notify", "", &[]); // has_return_value defaults false
        let out = emit(&inputs(&f, "thing"));
        assert!(
            out.contains("_lib._dispatch(\"thing-notify-post\", [])"),
            "void additive wrapper should use plain _dispatch for post:\n{out}",
        );
        assert!(
            !out.contains("_dispatch_post"),
            "void wrapper should not call _dispatch_post:\n{out}",
        );
    }

    #[test]
    fn coroutine_awaits_every_self_call() {
        let mut f = fixture("apply_async", "", &[]);
        f.is_coroutine = true;
        let out = emit(&inputs(&f, "worldsave"));
        // Void coroutine: 5 self-call sites, every one awaited.
        let awaited = out.matches("await self.apply_async()").count();
        assert_eq!(
            awaited, 5,
            "expected 5 awaited self-call sites in void coroutine additive wrapper; got {awaited}\n{out}",
        );
    }

    #[test]
    fn no_rename_prefix_leaks_into_output() {
        // Regression guard: the Inputs struct carries a `rename_prefix`
        // field inherited from the other templates, but additive must
        // never emit it (since it doesn't rename the vanilla body).
        let f = fixture("save_data", "dict", &["dict"]);
        let out = emit(&inputs(&f, "worldsave"));
        assert!(
            !out.contains("_rtv_vanilla_"),
            "additive template must not emit _rtv_vanilla_; got:\n{out}",
        );
    }

    #[test]
    fn four_space_indent_respected() {
        let f = fixture("save_data", "dict", &["dict"]);
        let mut ins = inputs(&f, "worldsave");
        ins.indent = "    ";
        let out = emit(&ins);
        assert!(!out.contains('\t'));
        assert!(out.contains("    var _lib"));
    }
}
