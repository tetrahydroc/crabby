//! Function-declaration model and body analysis.
//!
//! [`FuncDecl`] is the per-function shape the rewriter consumes. Body
//! analysis fills `is_coroutine` and `has_return_value` by scanning each
//! function's line range.

/// A single `func` declaration with everything the rewriter cares about.
///
/// The field set matches vostok's per-function dictionary in
/// `_rtv_parse_script` so ports stay mechanical. All fields are populated
/// by [`super::parser::parse_script`](super::parser::parse_script).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuncDecl {
    /// Function name exactly as declared.
    pub name: String,
    /// Raw parameter string between `(` and `)`.
    pub params: String,
    /// Parameter names, defaults + types stripped.
    pub param_names: Vec<String>,
    /// 1-based source line of the `func` line.
    pub line_number: u32,
    /// `true` if declared `static func`.
    pub is_static: bool,
    /// Explicit return type as written (e.g. `int`, `Array[int]`, `void`).
    /// `None` if no `->` annotation.
    pub return_type: Option<String>,
    /// `true` if the body contains an `await` expression.
    pub is_coroutine: bool,
    /// `true` if the function is expected to return a value.
    ///
    /// Set by: explicit return type other than `void`, or body containing
    /// `return <expr>` (not bare `return`). Explicit `void` return type
    /// forces `false` regardless of body.
    pub has_return_value: bool,
}

/// Inspect body lines `[start, end)` for `await` and `return <value>`.
pub fn analyze_body(lines: &[&str], start: usize, end: usize) -> (bool, bool) {
    let mut is_coroutine = false;
    let mut has_return_value = false;
    for line in &lines[start..end.min(lines.len())] {
        let trimmed = line.trim();
        if trimmed.contains("await ") {
            is_coroutine = true;
        }
        // "return <expr>", not bare "return", not "return#..." comment.
        if let Some(rest) = trimmed.strip_prefix("return ")
            && !rest.is_empty()
            && !rest.starts_with('#')
        {
            has_return_value = true;
        }
    }
    (is_coroutine, has_return_value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_await() {
        let lines = [
            "func foo():",
            "\tawait get_tree().process_frame",
            "\treturn",
        ];
        let (coroutine, has_ret) = analyze_body(&lines, 1, 3);
        assert!(coroutine);
        assert!(!has_ret);
    }

    #[test]
    fn detects_return_with_value() {
        let lines = ["func foo():", "\treturn 42"];
        let (coroutine, has_ret) = analyze_body(&lines, 1, 2);
        assert!(!coroutine);
        assert!(has_ret);
    }

    #[test]
    fn bare_return_is_not_value() {
        let lines = ["func foo():", "\treturn"];
        let (_, has_ret) = analyze_body(&lines, 1, 2);
        assert!(!has_ret);
    }

    #[test]
    fn empty_body_all_false() {
        let lines = ["func foo():"];
        let (coroutine, has_ret) = analyze_body(&lines, 1, 1);
        assert!(!coroutine);
        assert!(!has_ret);
    }

    #[test]
    fn await_in_middle_of_body_still_detected() {
        let lines = [
            "func foo():",
            "\tvar x = 1",
            "\tawait something()",
            "\treturn x",
        ];
        let (coroutine, has_ret) = analyze_body(&lines, 1, 4);
        assert!(coroutine);
        assert!(has_ret);
    }
}
