//! Per-method wrapper-template selection.
//!
//! Crabby emits one of several dispatch-wrapper templates per vanilla
//! method based on the script's characteristics and the method's body.
//! This module centralizes the decision so the rest of the rewriter
//! doesn't re-derive it.
//!
//! # Templates
//!
//! - [`TemplateKind::Additive`], `Resource`-subclass scripts whose
//!   method names are embedded in save files. Wrapper is added
//!   alongside the vanilla body under a distinct prefix; body stays
//!   unchanged so saves still deserialize. See
//!   [`template::additive`](crate::template::additive).
//! - [`TemplateKind::Fast`], per-frame / per-shot / short-lived
//!   scripts where dispatch overhead is visible in profile.
//!   Short-circuit, with dispatch only when at least one mod has
//!   registered a hook. See [`template::fast`](crate::template::fast).
//! - [`TemplateKind::Void`], void methods on ordinary scripts. Full
//!   dispatch pipeline with re-entry guard. See
//!   [`template::void`](crate::template::void).
//! - [`TemplateKind::NonVoid`], methods that return a value. Same as
//!   void but threads `_result` through every branch. See
//!   [`template::non_void`](crate::template::non_void).

use crabby_parser::{FuncDecl, ParsedScript};

use crate::engine_void::is_engine_void_method;
use crate::resource_serialized::is_additive_script;

/// Scripts that receive the fast wrapper template for every hookable
/// method. Named by filename (no path) so the detection is a string
/// comparison against `ParsedScript::filename`.
///
/// Drawn from vostok-mod-loader's `RTV_SKIP_LIST` (scripts vostok skipped
/// entirely for dispatch overhead reasons). Crabby keeps them hookable by
/// serving the fast template instead of dropping them.
pub const FAST_TEMPLATE_SCRIPTS: &[&str] = &["MuzzleFlash.gd", "Hit.gd", "Mine.gd"];

/// Which dispatch-wrapper template the rewriter should emit for a given
/// (script, method) pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateKind {
    /// Method on a resource-serialized script. Additive: vanilla body
    /// stays put under its original name; wrapper added alongside under
    /// [`ADDITIVE_HOOK_PREFIX`](crate::resource_serialized::ADDITIVE_HOOK_PREFIX).
    Additive,
    /// Void / no-return-value method on a [`FAST_TEMPLATE_SCRIPTS`] script.
    Fast,
    /// Void / no-return-value method on an ordinary script.
    Void,
    /// Method with a return value.
    NonVoid,
}

/// Pick the template for `func` on `script`.
///
/// Precedence:
///
/// 1. If the script is in
///    [`ADDITIVE_TEMPLATE_SCRIPTS`](crate::resource_serialized::ADDITIVE_TEMPLATE_SCRIPTS)
///    -> [`Additive`](TemplateKind::Additive). Additive applies regardless
///    of void-ness; the additive emitter picks the right body shape
///    internally based on the method's return status.
/// 2. Else if the script is in [`FAST_TEMPLATE_SCRIPTS`] ->
///    [`Fast`](TemplateKind::Fast). Non-void methods on a fast-targeted
///    script fall back to [`NonVoid`](TemplateKind::NonVoid) with a
///    tracing warning (authoring error).
/// 3. Else if the method is in the engine-void list **or** body analysis
///    found no `return <expr>` -> [`Void`](TemplateKind::Void).
/// 4. Else -> [`NonVoid`](TemplateKind::NonVoid).
#[must_use]
pub fn pick_template(script: &ParsedScript, func: &FuncDecl) -> TemplateKind {
    if is_additive_script(&script.filename) {
        return TemplateKind::Additive;
    }

    let is_void = is_engine_void_method(&func.name) || !func.has_return_value;

    if is_fast_script(&script.filename) {
        if is_void {
            return TemplateKind::Fast;
        }
        tracing::warn!(
            script = %script.filename,
            method = %func.name,
            "fast-templated script has non-void method; falling back to non-void template",
        );
        return TemplateKind::NonVoid;
    }

    if is_void {
        TemplateKind::Void
    } else {
        TemplateKind::NonVoid
    }
}

/// Whether `filename` is a [`FAST_TEMPLATE_SCRIPTS`] entry.
#[must_use]
pub fn is_fast_script(filename: &str) -> bool {
    FAST_TEMPLATE_SCRIPTS.contains(&filename)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(filename: &str) -> ParsedScript {
        ParsedScript {
            filename: filename.to_owned(),
            path: format!("res://Scripts/{filename}"),
            extends: None,
            class_name: None,
            var_names: Vec::new(),
            functions: Vec::new(),
            typed_decls: Vec::new(),
        }
    }

    fn func(name: &str, has_return_value: bool) -> FuncDecl {
        FuncDecl {
            name: name.to_owned(),
            params: String::new(),
            param_names: Vec::new(),
            line_number: 1,
            is_static: false,
            return_type: None,
            is_coroutine: false,
            has_return_value,
        }
    }

    #[test]
    fn void_method_on_ordinary_script() {
        let s = script("Controller.gd");
        let f = func("_physics_process", false);
        assert_eq!(pick_template(&s, &f), TemplateKind::Void);
    }

    #[test]
    fn non_void_method_on_ordinary_script() {
        let s = script("Controller.gd");
        let f = func("compute", true);
        assert_eq!(pick_template(&s, &f), TemplateKind::NonVoid);
    }

    #[test]
    fn engine_lifecycle_overrides_body_inference() {
        // `_ready` body that does `return <expr>`, engine list wins.
        let s = script("Controller.gd");
        let f = func("_ready", true);
        assert_eq!(pick_template(&s, &f), TemplateKind::Void);
    }

    #[test]
    fn fast_script_void_method_picks_fast() {
        for fname in FAST_TEMPLATE_SCRIPTS {
            let s = script(fname);
            let f = func("update", false);
            assert_eq!(
                pick_template(&s, &f),
                TemplateKind::Fast,
                "fast script {fname} should pick Fast",
            );
        }
    }

    #[test]
    fn fast_script_non_void_method_falls_back_to_non_void() {
        let s = script("MuzzleFlash.gd");
        let f = func("compute_intensity", true);
        assert_eq!(pick_template(&s, &f), TemplateKind::NonVoid);
    }

    #[test]
    fn fast_script_engine_lifecycle_picks_fast() {
        let s = script("MuzzleFlash.gd");
        let f = func("_process", false);
        assert_eq!(pick_template(&s, &f), TemplateKind::Fast);
    }

    #[test]
    fn is_fast_script_matches_exact_filename() {
        assert!(is_fast_script("MuzzleFlash.gd"));
        assert!(is_fast_script("Hit.gd"));
        assert!(is_fast_script("Mine.gd"));
        // Different casing or extension -> not a match.
        assert!(!is_fast_script("muzzleflash.gd"));
        assert!(!is_fast_script("MuzzleFlash"));
        assert!(!is_fast_script("Controller.gd"));
    }

    #[test]
    fn additive_script_void_method_picks_additive() {
        let s = script("WorldSave.gd");
        let f = func("save_data", false);
        assert_eq!(pick_template(&s, &f), TemplateKind::Additive);
    }

    #[test]
    fn additive_script_non_void_method_picks_additive() {
        let s = script("Validator.gd");
        let f = func("validate", true);
        // Additive applies irrespective of void-ness, the emitter picks
        // the right body shape internally.
        assert_eq!(pick_template(&s, &f), TemplateKind::Additive);
    }

    #[test]
    fn additive_overrides_fast_list() {
        // Hypothetical overlap: if a script ever appeared in both lists,
        // additive takes precedence because save compatibility trumps
        // per-frame overhead. This test guards the precedence rule.
        // None currently overlap in reality.
        let s = script("WorldSave.gd");
        let f = func("_process", false);
        // WorldSave isn't in the fast list, but even if it were, the
        // additive check runs first.
        assert_eq!(pick_template(&s, &f), TemplateKind::Additive);
    }
}
