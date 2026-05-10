//! Line-based `func` renaming + bare-`super()` rewriting.
//!
//! Given a target method name, walk the source line-by-line:
//!
//! 1. The top-level `func <target>(` line (no leading indent) becomes
//!    `func <rename_prefix><target>(`.
//! 2. Inside that method's body (subsequent indented lines), any bare
//!    `super(` or `super (` becomes `super.<target>(`. Explicit
//!    `super.Other()` passes through untouched.
//!
//! The body ends at the next top-level line (next `func`, top-level `var`,
//! etc.).

/// Rename `func <target>` to `func <rename_prefix><target>` and rewrite
/// bare `super()` calls inside that body to `super.<target>()`.
///
/// Other functions are untouched. Returns the modified source.
///
/// Single-method variant used by [`rewrite_single_method`].
/// See [`rename_many`] for the full-script pass.
///
/// [`rewrite_single_method`]: crate::rewrite_single_method
#[must_use]
pub fn rename_single(source: &str, target_method: &str, rename_prefix: &str) -> String {
    rename_many(source, std::slice::from_ref(&target_method), rename_prefix)
}

/// Rename every `func <name>` top-level declaration where `name` appears
/// in `target_methods` to `func <rename_prefix><name>`, and rewrite bare
/// `super()` calls inside each renamed body to `super.<name>()`.
///
/// `target_methods` is typically the set of hookable (non-static,
/// non-skipped) method names in the parsed script. Order doesn't matter;
/// lookup is via a local set built once.
///
/// Methods not in the set are untouched. Methods in the set but absent
/// from the source are silently no-op'd (mismatch surfaces later when the
/// rewriter tries to emit a wrapper that won't have a vanilla body to call).
#[must_use]
pub fn rename_many(source: &str, target_methods: &[&str], rename_prefix: &str) -> String {
    // HashSet lookup so the inner loop stays O(1) per top-level line.
    use std::collections::HashSet;
    let targets: HashSet<&str> = target_methods.iter().copied().collect();

    let mut lines: Vec<String> = source.split('\n').map(str::to_owned).collect();
    // Name of the currently-open renamed method body, if any. `None` means
    // either at top-level or inside an untouched body (the latter doesn't
    // matter; bare-super rewriting is scoped to renamed bodies).
    let mut inside_target: Option<String> = None;

    for line in &mut lines {
        // A top-level line (not indented) closes any open method body.
        let is_top_level = !line.is_empty() && !line.starts_with('\t') && !line.starts_with(' ');

        if is_top_level {
            inside_target = None;
            if let Some((renamed, target_name)) = try_rename_one_of(line, &targets, rename_prefix) {
                *line = renamed;
                inside_target = Some(target_name);
            }
            continue;
        }

        if let Some(name) = inside_target.as_deref()
            && line.contains("super")
        {
            *line = rewrite_bare_super(line, name);
        }
    }

    lines.join("\n")
}

/// Try renaming `line` as a top-level `func <NAME>(...)` declaration where
/// `<NAME>` is any of `targets`. Returns `(renamed_line, matched_name)` on
/// hit, or `None` otherwise.
fn try_rename_one_of(
    line: &str,
    targets: &std::collections::HashSet<&str>,
    rename_prefix: &str,
) -> Option<(String, String)> {
    let after_func = line.strip_prefix("func ")?;
    let open_paren = after_func.find('(')?;
    let (name_part, rest) = after_func.split_at(open_paren);
    let name = name_part.trim_end();
    if !targets.contains(name) {
        return None;
    }
    Some((format!("func {rename_prefix}{name}{rest}"), name.to_owned()))
}

/// If `line` is a top-level `func <target>(...)` declaration, return the
/// renamed version. Otherwise return `None`. Kept for the single-method
/// path + as a focused unit under test.
#[allow(dead_code)] // covered transitively by rename_many; kept as a focused test helper
fn try_rename_func(line: &str, target_method: &str, rename_prefix: &str) -> Option<String> {
    let after_func = line.strip_prefix("func ")?;
    let open_paren = after_func.find('(')?;
    let (name_part, rest) = after_func.split_at(open_paren);
    let name = name_part.trim_end();
    if name != target_method {
        return None;
    }
    Some(format!("func {rename_prefix}{target_method}{rest}"))
}

/// Rewrite bare `super(` to `super.<method_name>(` in one line, leaving
/// explicit `super.Other()` alone.
///
/// Edge cases handled:
/// - `super(` / `super (` -> `super.<method>(`
/// - `super.Something(` -> untouched
/// - `mysuper(` or `super_x(` (word containing 'super'), not `super` itself,
///   so skip
/// - `super` as a bare expression without following paren (rare), untouched
#[must_use]
fn rewrite_bare_super(line: &str, method_name: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if is_super_word_here(bytes, i) {
            // Found `super` as a whole word. Look past any spaces.
            let end = i + 5;
            let mut j = end;
            while j < bytes.len() && bytes[j] == b' ' {
                j += 1;
            }
            // If next non-space char is '(', it's a bare super call; rewrite.
            // If it's '.', it's already explicit; pass through.
            if j < bytes.len() && bytes[j] == b'(' {
                out.push_str("super.");
                out.push_str(method_name);
                // Skip original "super" + spaces so the '(' stays intact.
                i = j;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Check if position `i` starts the standalone word `super`. Requires the
/// preceding char (if any) and the char at `i+5` (if any) to not be
/// identifier-continuing.
fn is_super_word_here(bytes: &[u8], i: usize) -> bool {
    if i + 5 > bytes.len() {
        return false;
    }
    if &bytes[i..i + 5] != b"super" {
        return false;
    }
    // Word-boundary before.
    if i > 0 && is_ident_byte(bytes[i - 1]) {
        return false;
    }
    // Word-boundary after.
    if i + 5 < bytes.len() && is_ident_byte(bytes[i + 5]) {
        return false;
    }
    true
}

const fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_top_level_target() {
        let src = "\
extends Node

func _physics_process(delta):
\tpass

func other():
\tpass
";
        let out = rename_single(src, "_physics_process", "_rtv_vanilla_");
        assert!(out.contains("func _rtv_vanilla__physics_process(delta):"));
        assert!(out.contains("func other():")); // other functions untouched
    }

    #[test]
    fn rewrites_bare_super_inside_target_body() {
        let src = "\
func _physics_process(delta):
\tsuper(delta)
\tprint(\"done\")
";
        let out = rename_single(src, "_physics_process", "_rtv_vanilla_");
        assert!(out.contains("super._physics_process(delta)"));
        assert!(!out.contains("super(delta)"));
    }

    #[test]
    fn leaves_explicit_super_alone() {
        let src = "\
func _physics_process(delta):
\tsuper.OtherMethod(delta)
";
        let out = rename_single(src, "_physics_process", "_rtv_vanilla_");
        assert!(out.contains("super.OtherMethod(delta)"));
    }

    #[test]
    fn does_not_rewrite_super_in_other_function_bodies() {
        let src = "\
func _physics_process(delta):
\tpass

func other(delta):
\tsuper(delta)
";
        let out = rename_single(src, "_physics_process", "_rtv_vanilla_");
        // The `other` body's bare super stays, it's not the target method.
        assert!(out.contains("\tsuper(delta)"));
    }

    #[test]
    fn no_op_when_target_not_present() {
        let src = "\
func a():
\tpass
func b():
\tpass
";
        let out = rename_single(src, "nonexistent", "_rtv_vanilla_");
        assert_eq!(out, src);
    }

    #[test]
    fn does_not_match_partial_word_super() {
        let line = "\tvar mysuper = 1";
        assert_eq!(rewrite_bare_super(line, "m"), line);
    }

    #[test]
    fn tolerates_space_between_super_and_paren() {
        let line = "\tsuper (1, 2)";
        assert_eq!(rewrite_bare_super(line, "m"), "\tsuper.m(1, 2)");
    }

    #[test]
    fn rename_many_renames_every_listed_target() {
        let src = "\
extends Node

func a(x):
\tsuper(x)

func b():
\tpass

func c():
\tsuper()
";
        let out = rename_many(src, &["a", "c"], "_rtv_vanilla_");
        assert!(out.contains("func _rtv_vanilla_a(x):"));
        assert!(out.contains("func b():")); // not in target set
        assert!(out.contains("func _rtv_vanilla_c():"));
        // Super rewrites scoped per-body:
        assert!(out.contains("super.a(x)"));
        assert!(out.contains("super.c()"));
    }

    #[test]
    fn rename_many_scopes_super_rewrite_per_method() {
        // A's body calls super(); the rename should rewrite to super.a().
        // B isn't in the target set, so its super() stays untouched.
        let src = "\
func a():
\tsuper()

func b():
\tsuper()
";
        let out = rename_many(src, &["a"], "_rtv_vanilla_");
        assert!(out.contains("func _rtv_vanilla_a():\n\tsuper.a()"));
        // B's body is unchanged.
        assert!(out.contains("func b():\n\tsuper()"));
    }

    #[test]
    fn rename_many_empty_target_set_is_noop() {
        let src = "func a():\n\tpass\n";
        let out = rename_many(src, &[], "_rtv_vanilla_");
        assert_eq!(out, src);
    }
}
