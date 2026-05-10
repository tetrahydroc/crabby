//! Defensive autofix passes for vanilla source.
//!
//! Crabby's no-mod-rewriting policy means these passes run on vanilla
//! only. Vanilla RTV 4.6.1 was authored in Godot 4 and contains zero
//! legacy patterns in practice, so the passes are defensive against
//! future RTV updates and the edge-case vanilla idiom `if X: pass`
//! expanded into `if X:` with the body dropped.
//!
//! Currently ships one pass:
//!
//! - [`inject_pass_into_bodyless_blocks`], scans for block headers
//!   ending in `:` whose next meaningful line isn't more-indented, and
//!   inserts a `pass` line at `header_indent + one_indent_unit`.
//!
//! CRLF -> LF normalization lives in [`crate::normalize`] and is applied
//! earlier in the pipeline, so autofix passes here operate on
//! already-LF source.

/// Block-opening keywords that must be followed by an indented body.
///
/// `GDScript` treats `func`, `static func`, control flow, and class-scope
/// declarations as headers when they end in `:`. This list mirrors
/// vostok-mod-loader's autofix recognition set.
const BLOCK_KEYWORDS: &[&str] = &[
    "if ",
    "elif ",
    "else:",
    "for ",
    "while ",
    "match ",
    "func ",
    "static func ",
    "class ",
];

/// Scan `source` for block headers whose body is missing and inject a
/// `pass` line at the expected indent. Returns the (possibly unchanged)
/// source and a count of injections made.
///
/// `indent_unit` is what one indent-level looks like in this script (a
/// tab or N spaces), detected earlier by the `normalize` module's
/// indent-style detector and passed in so the injected line matches the
/// file's existing style.
#[must_use]
pub fn inject_pass_into_bodyless_blocks(source: &str, indent_unit: &str) -> (String, usize) {
    let lines: Vec<&str> = source.split('\n').collect();
    let mut out = String::with_capacity(source.len());
    let mut injections = 0usize;

    for i in 0..lines.len() {
        let line = lines[i];
        out.push_str(line);
        if i + 1 < lines.len() {
            out.push('\n');
        }

        let Some(header_indent) = block_header_indent(line) else {
            continue;
        };

        // Find the next non-blank non-comment line.
        let mut j = i + 1;
        while j < lines.len() {
            let next = lines[j];
            let trimmed = next.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                j += 1;
                continue;
            }
            break;
        }

        // Body is missing if:
        //   - end of file reached without finding a line
        //   - OR the next meaningful line's indent is <= header's
        let next_indent = lines.get(j).and_then(|l| leading_indent_width(l));
        let needs_pass = next_indent.is_none_or(|n| n <= header_indent);
        if !needs_pass {
            continue;
        }

        // Inject `<header_indent_bytes><one indent_unit>pass` AFTER the
        // header line (already pushed above). The injected line
        // sits immediately after the header's newline; the following
        // blank/comment lines (if any) remain in place.
        let header_indent_prefix: String = line.chars().take(header_indent).collect();
        out.push_str(&header_indent_prefix);
        out.push_str(indent_unit);
        out.push_str("pass\n");
        injections += 1;
    }

    (out, injections)
}

/// Return the leading-indent **character count** of `line` if it begins
/// with a valid block header (keyword from [`BLOCK_KEYWORDS`], ends with
/// `:` ignoring trailing whitespace and trailing `# comment`), else `None`.
///
/// Indent width is measured in characters (tabs and spaces each count
/// as 1). Vostok's approach is equivalent: its
/// `_rtv_leading_indent` returns the substring and compares substrings;
/// here, character counts are compared instead since the prefix must be
/// rebuilt for the injected line.
fn block_header_indent(line: &str) -> Option<usize> {
    // Must end with `:` (after stripping trailing comment + whitespace).
    if !line_ends_with_colon(line) {
        return None;
    }

    // Leading indent.
    let indent_chars = line.chars().take_while(|c| *c == '\t' || *c == ' ').count();
    let trimmed = &line[indent_chars..];

    if !BLOCK_KEYWORDS.iter().any(|kw| trimmed.starts_with(kw)) {
        return None;
    }

    Some(indent_chars)
}

/// Return true if `line`, after stripping any trailing `# comment` and
/// trailing whitespace, ends with `:`.
fn line_ends_with_colon(line: &str) -> bool {
    let code = strip_trailing_comment(line);
    code.trim_end().ends_with(':')
}

/// Strip `# comment` from the end of a line **outside** string literals.
/// Handles `"..."` and `'...'` with backslash escapes.
fn strip_trailing_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match (quote, b) {
            (Some(_), b'\\') if i + 1 < bytes.len() => {
                // Skip the escaped character, it never closes the string.
                i += 2;
            }
            (Some(q), c) if q == c => {
                quote = None;
                i += 1;
            }
            (None, b'"' | b'\'') => {
                quote = Some(b);
                i += 1;
            }
            (None, b'#') => {
                return &line[..i];
            }
            _ => {
                i += 1;
            }
        }
    }
    line
}

/// Width of the leading-whitespace run on `line`, counting tabs and
/// spaces as 1 each. Returns `None` for a blank line (treated as "no
/// body yet, keep looking").
fn leading_indent_width(line: &str) -> Option<usize> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(line.chars().take_while(|c| *c == '\t' || *c == ' ').count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_op_on_clean_source() {
        let src = "\
extends Node

func foo():
\tpass

if true:
\tprint(\"ok\")
";
        let (out, n) = inject_pass_into_bodyless_blocks(src, "\t");
        assert_eq!(n, 0);
        assert_eq!(out, src);
    }

    #[test]
    fn injects_pass_when_if_body_missing() {
        let src = "\
func foo():
\tif true:
\treturn
";
        // `if true:` at 1 tab; next line `\treturn` is ALSO at 1 tab, so
        // `if` has no body. Inject `\t\tpass`.
        let (out, n) = inject_pass_into_bodyless_blocks(src, "\t");
        assert_eq!(n, 1);
        assert!(out.contains("\tif true:\n\t\tpass\n\treturn\n"));
    }

    #[test]
    fn injects_pass_for_empty_func_body() {
        let src = "\
func foo():

func bar():
\tpass
";
        let (out, n) = inject_pass_into_bodyless_blocks(src, "\t");
        assert_eq!(n, 1);
        // `func foo():` followed only by a blank then `func bar():`, so
        // no indented body, inject `pass` right after foo.
        assert!(out.contains("func foo():\n\tpass\n\nfunc bar():"));
    }

    #[test]
    fn injects_pass_when_file_ends_on_header() {
        let src = "func foo():\n";
        let (out, n) = inject_pass_into_bodyless_blocks(src, "\t");
        assert_eq!(n, 1);
        assert_eq!(out, "func foo():\n\tpass\n");
    }

    #[test]
    fn uses_configured_indent_unit() {
        let src = "func foo():\nfunc bar():\n\tpass\n";
        let (out, _) = inject_pass_into_bodyless_blocks(src, "    ");
        // Injected line uses 4 spaces (config), not tab.
        assert!(out.contains("func foo():\n    pass\n"));
    }

    #[test]
    fn skips_blank_and_comment_lines_when_searching_for_body() {
        let src = "\
if true:
\t# waiting to implement
\tpass
";
        // Body is `\tpass` at indent 1; header `if` at indent 0. Body
        // deeper than header -> no injection.
        let (out, n) = inject_pass_into_bodyless_blocks(src, "\t");
        assert_eq!(n, 0);
        assert_eq!(out, src);
    }

    #[test]
    fn does_not_misfire_on_ternary_if_expression() {
        // `x = foo if bar else baz` has no trailing `:`.
        let src = "var x = foo if bar else baz\nprint(x)\n";
        let (out, n) = inject_pass_into_bodyless_blocks(src, "\t");
        assert_eq!(n, 0);
        assert_eq!(out, src);
    }

    #[test]
    fn does_not_misfire_on_dict_literal_with_colon() {
        // `var d = {"k": v}` has a `:` but no block keyword.
        let src = "var d = {\"k\": v}\nprint(d)\n";
        let (out, n) = inject_pass_into_bodyless_blocks(src, "\t");
        assert_eq!(n, 0);
        assert_eq!(out, src);
    }

    #[test]
    fn handles_trailing_comment_after_colon() {
        // `func foo(): # docstring` is still a valid header.
        let src = "func foo(): # docstring\nfunc bar():\n\tpass\n";
        let (out, n) = inject_pass_into_bodyless_blocks(src, "\t");
        assert_eq!(n, 1);
        assert!(out.contains("func foo(): # docstring\n\tpass\n"));
    }

    #[test]
    fn does_not_treat_hash_in_string_as_comment() {
        let src = "if x == \"#\":\n\tpass\n";
        let (out, n) = inject_pass_into_bodyless_blocks(src, "\t");
        // Body present; no injection.
        assert_eq!(n, 0);
        assert_eq!(out, src);
    }

    #[test]
    fn injects_multiple_passes_in_same_scan() {
        let src = "\
func a():
func b():
\tpass
";
        let (out, n) = inject_pass_into_bodyless_blocks(src, "\t");
        assert_eq!(n, 1); // only `a` is bodyless
        assert!(out.contains("func a():\n\tpass\nfunc b():"));
    }

    #[test]
    fn handles_static_func_header() {
        let src = "static func foo():\nstatic func bar():\n\tpass\n";
        let (out, n) = inject_pass_into_bodyless_blocks(src, "\t");
        assert_eq!(n, 1);
        assert!(out.contains("static func foo():\n\tpass\n"));
    }

    #[test]
    fn handles_class_header() {
        let src = "class Inner:\nclass Other:\n\tvar x = 1\n";
        let (out, n) = inject_pass_into_bodyless_blocks(src, "\t");
        assert_eq!(n, 1);
        assert!(out.contains("class Inner:\n\tpass\nclass Other:"));
    }

    #[test]
    fn handles_else_header() {
        // `else:` is the one keyword in BLOCK_KEYWORDS that has the
        // colon baked into the match prefix. Make sure a bodyless
        // `else:` block still triggers.
        let src = "\
func foo():
\tif x:
\t\treturn 1
\telse:
\treturn 2
";
        let (out, n) = inject_pass_into_bodyless_blocks(src, "\t");
        // `else:` at 1 tab, next line `\treturn 2` also at 1 tab -> inject.
        assert_eq!(n, 1);
        assert!(out.contains("\telse:\n\t\tpass\n\treturn 2"));
    }

    #[test]
    fn strip_trailing_comment_preserves_strings() {
        assert_eq!(
            strip_trailing_comment("print(\"# not a comment\") # real"),
            "print(\"# not a comment\") "
        );
        assert_eq!(strip_trailing_comment("var x = 1 # trailing"), "var x = 1 ");
        assert_eq!(strip_trailing_comment("no comment here"), "no comment here");
    }

    #[test]
    fn leading_indent_width_handles_mixed_and_blanks() {
        assert_eq!(leading_indent_width("\t\tfoo"), Some(2));
        assert_eq!(leading_indent_width("    foo"), Some(4));
        assert_eq!(leading_indent_width("foo"), Some(0));
        assert_eq!(leading_indent_width(""), None);
        assert_eq!(leading_indent_width("   \t"), None); // whitespace-only
    }
}
