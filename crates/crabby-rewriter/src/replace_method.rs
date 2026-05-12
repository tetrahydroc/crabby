//! Mod-supplied `replace_method` splice: swap one `func <name>` block
//! inside an existing target script for a foreign source.
//!
//! Bake-time only. Driven by the `["replace_method", target, name,
//! source]` setup-plan verb (see crabby-mod-analyzer's `OverlayVerb`).
//! Runs before the rewriter's normal pass-1 transforms, so the splice
//! result feeds into wrapper synthesis like vanilla source would, the
//! mod's replacement body becomes the "vanilla body" the wrapper calls.
//!
//! # Splice algorithm
//!
//! Input is the target script's source plus the method name and the
//! foreign source the mod ships under `overlays/`. The foreign source
//! is expected to contain exactly one full `func <name>(...) :` block
//! (signature + indented body); we do NOT split it out by name on the
//! foreign side, the mod author hands us the whole replacement.
//!
//! 1. Walk target source line-by-line until we find the line that
//!    begins with `func <name>(` at the same indent depth as the rest
//!    of the target's top-level (or class-level) declarations.
//! 2. Scan forward to find the end of that function's body: the first
//!    non-empty line whose indent is `<=` the `func` line's indent.
//!    Blank/whitespace-only lines are skipped (they don't terminate
//!    the body).
//! 3. Replace the byte span `[func_line_start, body_end_exclusive)`
//!    with the foreign source. Caller is responsible for trimming
//!    trailing newlines on the foreign source if they care about
//!    output formatting; we splice verbatim.

use crabby_error::{CrabbyError, Result};

/// Splice `foreign_source` into `target_source` at the position of
/// `func <method_name>` (replacing the existing function definition).
///
/// # Errors
///
/// - [`CrabbyError::Rewrite`] if no `func <method_name>` declaration
///   is found in `target_source`. The bake surfaces this as a hard
///   failure so authors don't ship a `replace_method` against a
///   method that vanilla doesn't have.
/// - [`CrabbyError::Rewrite`] when the foreign source's body uses a
///   different indent character (tab vs space) than the target's
///   body. GDScript rejects mixed indentation in a single file with
///   "Used tab character for indentation instead of space as used
///   before in the file" (or vice versa); refusing the splice here
///   surfaces the bug at bake time instead of at the player's first
///   trigger of the spliced code path.
pub fn splice_method(
    target_source: &str,
    method_name: &str,
    foreign_source: &str,
) -> Result<String> {
    let span =
        locate_method_span(target_source, method_name).ok_or_else(|| CrabbyError::Rewrite {
            context: format!("replace_method: target has no `func {method_name}`"),
            source: "method not found in target source".into(),
        })?;

    // Indent-character compatibility check. Both the target's existing
    // body indent and the foreign source's body indent need to be the
    // same character (tab vs space). When either side has no indented
    // body lines (e.g. `func empty(): pass` on one line, or a foreign
    // source consisting solely of a header), there's nothing to
    // mismatch and the check passes through.
    if let (Some(target_kind), Some(foreign_kind)) = (
        first_body_indent_kind(target_source),
        first_body_indent_kind(foreign_source),
    ) && target_kind != foreign_kind
    {
        return Err(CrabbyError::Rewrite {
            context: format!(
                "replace_method: foreign source for `{method_name}` indents with {} but target file indents with {}; GDScript will refuse to parse the post-splice file",
                foreign_kind.describe(),
                target_kind.describe(),
            ),
            source: "indent character mismatch between foreign source and target".into(),
        });
    }

    let mut out = String::with_capacity(
        target_source.len() - (span.end - span.start) + foreign_source.len() + 1,
    );
    out.push_str(&target_source[..span.start]);
    out.push_str(foreign_source);
    // Ensure a trailing newline between the foreign source and whatever
    // comes after, otherwise the next line gets jammed onto the foreign
    // source's last line. Skip when the splice already ends with one.
    if !foreign_source.ends_with('\n') && span.end < target_source.len() {
        out.push('\n');
    }
    out.push_str(&target_source[span.end..]);
    Ok(out)
}

/// Indent character used for body lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndentKind {
    Tab,
    Space,
}

impl IndentKind {
    fn describe(self) -> &'static str {
        match self {
            Self::Tab => "tabs",
            Self::Space => "spaces",
        }
    }
}

/// Sniff the first indented line in `source` and return its leading
/// character. Returns `None` when no line in the file starts with
/// either a tab or a space (file has no indented bodies at all, e.g.
/// header-only or all-top-level declarations).
///
/// "Indented" here means the line begins with `\t` or ` `. Mixed
/// runs (a tab followed by spaces) report on the FIRST character,
/// which is what GDScript's parser keys on for its homogeneity rule.
fn first_body_indent_kind(source: &str) -> Option<IndentKind> {
    for line in source.lines() {
        match line.as_bytes().first() {
            Some(b'\t') => return Some(IndentKind::Tab),
            Some(b' ') => return Some(IndentKind::Space),
            // Empty line or non-whitespace start: keep scanning. A
            // single empty line in the middle of an otherwise
            // tab-indented file mustn't terminate the search.
            _ => continue,
        }
    }
    None
}

/// Byte span of one `func` block: `[start, end)` covering the `func`
/// header line through the last line of the indented body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FuncSpan {
    start: usize,
    end: usize,
}

/// Find the byte span of `func <method_name>(...)` and its body. None
/// if no matching declaration is present.
///
/// Indent-agnostic: matches `func` at any leading-whitespace depth so
/// inner-class methods (declared at +1 tab inside a `class Foo:` block)
/// work the same as top-level ones. The body extends until the first
/// non-empty line whose indent is `<=` the `func` line's indent.
fn locate_method_span(source: &str, method_name: &str) -> Option<FuncSpan> {
    // Walk lines tracking byte offsets. `split_inclusive('\n')` keeps
    // the trailing newline on each item so offset bookkeeping is
    // exact and the span we return covers full lines.
    let mut header_start: Option<usize> = None;
    let mut header_indent: usize = 0;
    let mut last_body_end: usize = 0;

    let mut cursor = 0usize;
    for line in source.split_inclusive('\n') {
        let line_start = cursor;
        cursor += line.len();
        let line_end = cursor;

        if header_start.is_none() {
            // Looking for the header.
            let indent = leading_indent(line);
            let trimmed = &line[indent..];
            if is_func_decl(trimmed, method_name) {
                header_start = Some(line_start);
                header_indent = indent;
                last_body_end = line_end;
            }
        } else {
            // Inside the body. A blank/whitespace-only line doesn't
            // close the body (GDScript permits blank lines inside a
            // function). A non-empty line whose indent is `<=` the
            // header's indent closes it.
            if is_blank_or_comment_only_outside_body(line) {
                // Don't extend last_body_end across blank lines: the
                // run of trailing blank lines AFTER the body should
                // belong to whatever comes next (or to the file
                // tail), not to this function. Only extend on
                // properly-indented body lines.
                continue;
            }
            let indent = leading_indent(line);
            if indent <= header_indent {
                // Top of next declaration. Body ended at last_body_end
                // (which only advanced on indented body lines).
                break;
            }
            last_body_end = line_end;
        }
    }

    let start = header_start?;
    Some(FuncSpan {
        start,
        end: last_body_end,
    })
}

/// Count the number of leading whitespace bytes (tabs + spaces) on a
/// line. Treats tabs and spaces equally for ordering purposes; that's
/// fine since GDScript style is consistent within any one file (you
/// can't legally mix tabs and spaces in the same indent depth).
fn leading_indent(line: &str) -> usize {
    line.bytes()
        .take_while(|b| *b == b'\t' || *b == b' ')
        .count()
}

/// True when `trimmed` (line with leading indent already stripped)
/// starts a `func <method_name>(` declaration.
///
/// Handles `static func` by stripping the modifier first. Tolerant of
/// extra whitespace between `func` and the name.
fn is_func_decl(trimmed: &str, method_name: &str) -> bool {
    let after_static = trimmed.strip_prefix("static ").unwrap_or(trimmed);
    let Some(after_func) = after_static.strip_prefix("func") else {
        return false;
    };
    // Require at least one whitespace between `func` and the name,
    // otherwise `funcname` would match.
    let after_func = after_func.trim_start_matches([' ', '\t']);
    if after_func.len() == after_static.len() - "func".len() {
        return false; // no separator
    }
    let Some(rest) = after_func.strip_prefix(method_name) else {
        return false;
    };
    // The character right after the name must be `(` (possibly preceded
    // by whitespace) so we don't half-match on a prefix.
    let rest = rest.trim_start_matches([' ', '\t']);
    rest.starts_with('(')
}

/// True when the line is blank or contains only a comment. Used to
/// skip over runs of blank lines inside a function body without
/// closing the body span.
fn is_blank_or_comment_only_outside_body(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty() || trimmed.starts_with('#')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splices_simple_method_at_top_level() {
        let target = "\
extends Node

func _ready() -> void:
\tprint(\"hello\")
\tprint(\"world\")

func other() -> int:
\treturn 42
";
        let foreign = "func _ready() -> void:\n\tprint(\"replaced\")\n";
        let out = splice_method(target, "_ready", foreign).unwrap();
        assert!(out.contains("print(\"replaced\")"));
        assert!(!out.contains("print(\"hello\")"));
        assert!(out.contains("func other()"));
        assert!(out.contains("return 42"));
    }

    #[test]
    fn splices_last_method_in_file() {
        let target = "\
extends Node

func only_one() -> int:
\treturn 1
";
        let foreign = "func only_one() -> int:\n\treturn 99\n";
        let out = splice_method(target, "only_one", foreign).unwrap();
        assert!(out.contains("return 99"));
        assert!(!out.contains("return 1\n"));
    }

    #[test]
    fn errors_when_method_missing() {
        let target = "\
extends Node

func _ready() -> void:
\tpass
";
        let foreign = "func nope():\n\tpass\n";
        let err = splice_method(target, "nope", foreign).expect_err("missing method");
        match err {
            CrabbyError::Rewrite { context, .. } => {
                assert!(context.contains("nope"));
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn preserves_blank_lines_inside_body() {
        let target = "\
extends Node

func with_blank() -> void:
\tprint(\"a\")

\tprint(\"b\")

func after() -> void:
\tpass
";
        let foreign = "func with_blank() -> void:\n\tprint(\"replaced\")\n";
        let out = splice_method(target, "with_blank", foreign).unwrap();
        assert!(out.contains("func after()"));
        assert!(out.contains("print(\"replaced\")"));
        assert!(!out.contains("print(\"a\")"));
        assert!(!out.contains("print(\"b\")"));
    }

    #[test]
    fn handles_static_func() {
        let target = "\
extends Node

static func helper(x: int) -> int:
\treturn x * 2

func use_it() -> void:
\thelper(3)
";
        let foreign = "static func helper(x: int) -> int:\n\treturn x * 10\n";
        let out = splice_method(target, "helper", foreign).unwrap();
        assert!(out.contains("return x * 10"));
        assert!(!out.contains("return x * 2"));
    }

    #[test]
    fn handles_inner_class_method() {
        let target = "\
extends Node

class Inner:
\tvar x: int = 0

\tfunc tick() -> void:
\t\tx += 1

\tfunc other() -> void:
\t\tpass
";
        let foreign = "\tfunc tick() -> void:\n\t\tx += 100\n";
        let out = splice_method(target, "tick", foreign).unwrap();
        assert!(out.contains("x += 100"));
        assert!(!out.contains("x += 1\n"));
        assert!(out.contains("func other()"));
    }

    #[test]
    fn does_not_match_method_name_prefix() {
        let target = "\
extends Node

func ready_extra() -> void:
\tpass

func ready() -> void:
\tprint(\"original\")
";
        let foreign = "func ready() -> void:\n\tprint(\"new\")\n";
        let out = splice_method(target, "ready", foreign).unwrap();
        assert!(out.contains("func ready_extra()"));
        assert!(out.contains("print(\"new\")"));
        assert!(!out.contains("print(\"original\")"));
    }

    #[test]
    fn appends_newline_when_foreign_source_lacks_one() {
        let target = "func a() -> void:\n\tpass\n\nfunc b() -> void:\n\tpass\n";
        let foreign = "func a() -> void:\n\tpass # changed";
        let out = splice_method(target, "a", foreign).unwrap();
        // `func b()` should still appear on its own line.
        assert!(out.contains("# changed\nfunc b()") || out.contains("# changed\n\nfunc b()"));
    }

    #[test]
    fn errors_when_foreign_uses_spaces_against_tab_target() {
        let target = "\
extends Node

func _ready() -> void:
\tprint(\"hello\")
";
        // 4-space body indent against a tab-indented target.
        let foreign = "func _ready() -> void:\n    print(\"replaced\")\n";
        let err = splice_method(target, "_ready", foreign).expect_err("indent mismatch");
        match err {
            CrabbyError::Rewrite { context, .. } => {
                assert!(context.contains("spaces"), "context: {context}");
                assert!(context.contains("tabs"), "context: {context}");
                assert!(context.contains("_ready"), "context: {context}");
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn errors_when_foreign_uses_tabs_against_space_target() {
        let target = "\
extends Node

func _ready() -> void:
    print(\"hello\")
";
        let foreign = "func _ready() -> void:\n\tprint(\"replaced\")\n";
        let err = splice_method(target, "_ready", foreign).expect_err("indent mismatch");
        match err {
            CrabbyError::Rewrite { context, .. } => {
                assert!(context.contains("tabs"), "context: {context}");
                assert!(context.contains("spaces"), "context: {context}");
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[test]
    fn passes_when_foreign_has_no_indented_lines() {
        // Foreign source is a single-line `func` (no body indent at
        // all). Nothing to mismatch.
        let target = "\
extends Node

func helper() -> int:
\treturn 1

func other() -> void:
\tpass
";
        let foreign = "func helper() -> int: return 99\n";
        let out = splice_method(target, "helper", foreign).unwrap();
        assert!(out.contains("return 99"));
    }

    #[test]
    fn passes_when_target_has_no_indented_lines() {
        // Pathological target with only a single-line func.
        let target = "extends Node\nfunc helper() -> int: return 1\n";
        let foreign = "func helper() -> int:\n\treturn 99\n";
        let out = splice_method(target, "helper", foreign).unwrap();
        assert!(out.contains("return 99"));
    }
}
