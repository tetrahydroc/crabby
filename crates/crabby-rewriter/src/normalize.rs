//! Source normalization helpers applied before rewrite: line-ending
//! normalization and indent-style detection.
//!
//! Mirrors vostok-mod-loader's `_detect_indent_style` and the one-liner
//! CRLF strip vostok does at the top of `_rtv_rewrite_vanilla_source`.

/// Normalize line endings: `\r\n` → `\n`, stray `\r` → `\n`.
///
/// `GDScript` rejects files that mix CRLF and LF with a misleading
/// indentation error. The rewriter appends LF-only wrappers, so the
/// whole file must be LF before those appends happen.
#[must_use]
pub fn normalize_line_endings(source: &str) -> String {
    source.replace("\r\n", "\n").replace('\r', "\n")
}

/// Detect the source's indent unit.
///
/// Returns `"\t"` if the first indented non-blank non-comment line starts
/// with a tab, `" "` repeated N times if it starts with N spaces, or
/// `"\t"` as the default when nothing in the file is indented.
#[must_use]
pub fn detect_indent_style(source: &str) -> String {
    for line in source.split('\n') {
        let Some(first_char) = line.chars().next() else {
            continue;
        };
        if first_char != '\t' && first_char != ' ' {
            continue;
        }
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }
        if first_char == '\t' {
            return "\t".to_owned();
        }
        let n = line.chars().take_while(|c| *c == ' ').count();
        if n > 0 {
            return " ".repeat(n);
        }
    }
    "\t".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_endings_crlf_normalized() {
        assert_eq!(normalize_line_endings("a\r\nb\r\n"), "a\nb\n");
    }

    #[test]
    fn line_endings_lone_cr_normalized() {
        assert_eq!(normalize_line_endings("a\rb"), "a\nb");
    }

    #[test]
    fn line_endings_mixed_normalized() {
        assert_eq!(normalize_line_endings("a\r\nb\rc\nd"), "a\nb\nc\nd");
    }

    #[test]
    fn line_endings_pure_lf_unchanged() {
        assert_eq!(normalize_line_endings("a\nb\n"), "a\nb\n");
    }

    #[test]
    fn indent_tab_detected() {
        let src = "func foo():\n\treturn 1\n";
        assert_eq!(detect_indent_style(src), "\t");
    }

    #[test]
    fn indent_four_spaces_detected() {
        let src = "func foo():\n    return 1\n";
        assert_eq!(detect_indent_style(src), "    ");
    }

    #[test]
    fn indent_two_spaces_detected() {
        let src = "func foo():\n  return 1\n";
        assert_eq!(detect_indent_style(src), "  ");
    }

    #[test]
    fn indent_skips_blank_and_comment_lines() {
        // First indented line is an indented comment; skip and use the
        // next real indented line.
        let src = "\
func foo():
\t# blank comment indented with tabs
    return 1
";
        // The comment line is indented with a tab but is a comment (starts
        // with `#`), so skip. Next line uses 4 spaces → detect 4 spaces.
        assert_eq!(detect_indent_style(src), "    ");
    }

    #[test]
    fn indent_defaults_to_tab_when_no_indented_lines() {
        assert_eq!(detect_indent_style("class_name Foo\n"), "\t");
    }

    #[test]
    fn indent_deepest_indent_not_first_wins_if_first_is_comment() {
        let src = "\
func foo():
\t# comment
\tpass
";
        // Comment skipped, the real `pass` line uses tab → "\t".
        assert_eq!(detect_indent_style(src), "\t");
    }
}
