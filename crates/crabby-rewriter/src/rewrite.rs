//! Single-method rewrite orchestrator.
//!
//! Rewrites exactly one void method on one script. Predates the
//! full-script rewriter (`rewrite_full_script`) and is retained for
//! callers that want a focused single-method pass.

use crabby_error::{CrabbyError, Result};
use crabby_parser::ParsedScript;

use crate::engine_void::is_engine_void_method;
use crate::renamer::rename_single;
use crate::template::void::{self, Inputs};

/// Rewrite one method on a parsed script.
///
/// The method must be void (either an engine-lifecycle method or a
/// `-> void` annotated / no-return-value body). This entry point does
/// not emit the non-void or await-aware templates.
///
/// # Errors
///
/// - [`CrabbyError::Rewrite`] if `target_method` isn't present in `parsed`
/// - [`CrabbyError::Rewrite`] if the target is `is_static`
///   (static methods are not hookable; wrapping them would leak a `self`
///   reference that doesn't exist)
/// - [`CrabbyError::Rewrite`] if the target is non-void
///   (out of scope; use the non-void template via the full-script entry point)
///
/// [`CrabbyError::Rewrite`]: crabby_error::CrabbyError::Rewrite
pub fn rewrite_single_method(
    source: &str,
    parsed: &ParsedScript,
    target_method: &str,
    script_prefix: &str,
    indent: &str,
) -> Result<String> {
    let func = parsed
        .functions
        .iter()
        .find(|f| f.name == target_method)
        .ok_or_else(|| CrabbyError::Rewrite {
            context: format!(
                "{}: target method `{target_method}` not found",
                parsed.filename,
            ),
            source: "no match in parsed.functions".into(),
        })?;

    if func.is_static {
        return Err(CrabbyError::Rewrite {
            context: format!(
                "{}: cannot wrap static method `{target_method}`",
                parsed.filename,
            ),
            source: "static methods are not hookable".into(),
        });
    }

    let is_void = is_engine_void_method(&func.name) || !func.has_return_value;
    if !is_void {
        return Err(CrabbyError::Rewrite {
            context: format!(
                "{}: method `{target_method}` is non-void; use the non-void template",
                parsed.filename,
            ),
            source: "this entry point implements only the void template".into(),
        });
    }

    // 1. Normalize CRLF -> LF. Mirrors vostok's pre-rewrite normalization.
    let src = source.replace("\r\n", "\n").replace('\r', "\n");

    // 2. Rename `func <target>` -> `func _rtv_vanilla_<target>` + bare-super rewrite.
    let renamed = rename_single(&src, target_method, "_rtv_vanilla_");

    // 3. Append the dispatch wrapper at EOF.
    let wrapper = void::emit(&Inputs {
        func,
        script_prefix,
        indent,
        rename_prefix: "_rtv_vanilla_",
        flags: crate::HookFlags::all(),
    });

    let mut out = String::with_capacity(renamed.len() + wrapper.len() + 64);
    out.push_str(&renamed);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("\n# --- Crabby hook dispatch wrapper ---\n");
    out.push_str(&wrapper);
    Ok(out)
}
