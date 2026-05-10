//! Top-level script parser.
//!
//! Two-pass scan:
//! 1. Line-by-line: detect `extends`, `class_name`, top-level `var` names,
//!    and every `func` / `static func` declaration with its line number.
//! 2. Per-function: determine `is_coroutine` and `has_return_value` by
//!    examining body lines up to the next function's line number.

use crabby_error::Result;

use crate::function::{FuncDecl, analyze_body};
use crate::params::{extract_param_names, extract_typed_params};
use crate::regex_set::{CLASS_NAME, EXTENDS, FUNC, INFERRED_NEW_VAR, STATIC_FUNC, TYPED_VAR, VAR};
use crate::typed_decls::{DeclScope, TypedDecl};

/// A parsed `GDScript` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedScript {
    /// Bare filename (e.g. `"Hitbox.gd"`).
    pub filename: String,
    /// Canonical resource path (e.g. `"res://Scripts/Hitbox.gd"`).
    pub path: String,
    /// Base class name or path from `extends`, if declared.
    pub extends: Option<String>,
    /// `class_name` value if declared.
    pub class_name: Option<String>,
    /// Top-level `var` names (excluding indented declarations).
    pub var_names: Vec<String>,
    /// All function declarations, in source order.
    pub functions: Vec<FuncDecl>,
    /// Variables and parameters with explicit (or trivially inferred)
    /// type annotations. Used by the consumer-call rewriter to type
    /// receivers; see [`TypedDecl`] / [`DeclScope`] for the model.
    pub typed_decls: Vec<TypedDecl>,
}

/// Parse a `GDScript` source file.
///
/// Pure over `filename` + `source`; returns an error only if a regex fails
/// to compile (which can't happen since our patterns are static). Kept as
/// `Result` so future validation passes can surface errors.
pub fn parse_script(filename: &str, source: &str) -> Result<ParsedScript> {
    let lines: Vec<&str> = source.split('\n').collect();
    let mut state = ScanState::default();

    for (line_num, line) in lines.iter().enumerate() {
        scan_line(line, line_num, &mut state);
    }

    // Second pass: body analysis.
    let mut functions = Vec::with_capacity(state.func_starts.len());
    for (idx, fs) in state.func_starts.iter().enumerate() {
        let body_start = fs.line_num + 1;
        let body_end = state
            .func_starts
            .get(idx + 1)
            .map_or(lines.len(), |next| next.line_num);
        let (is_coroutine, mut has_return_value) = analyze_body(&lines, body_start, body_end);

        if let Some(ret) = fs.return_type.as_deref() {
            has_return_value = ret != "void";
        }

        functions.push(FuncDecl {
            name: fs.name.clone(),
            params: fs.params.clone(),
            param_names: fs.param_names.clone(),
            line_number: u32::try_from(fs.line_num + 1).unwrap_or(u32::MAX),
            is_static: fs.is_static,
            return_type: fs.return_type.clone(),
            is_coroutine,
            has_return_value,
        });
    }

    Ok(ParsedScript {
        filename: filename.to_owned(),
        path: format!("res://Scripts/{filename}"),
        extends: state.extends,
        class_name: state.class_name,
        var_names: state.var_names,
        functions,
        typed_decls: state.typed_decls,
    })
}

/// Accumulator for [`scan_line`]. Mirrors the original local variables
/// of `parse_script` 1:1, refactored into a struct so the per-line
/// scanning can be a stand-alone helper without sprawling parameter lists.
#[derive(Default)]
struct ScanState {
    var_names: Vec<String>,
    typed_decls: Vec<TypedDecl>,
    extends: Option<String>,
    class_name: Option<String>,
    func_starts: Vec<FuncStart>,
    /// 1-based line of the most recent `func` declaration that's still
    /// in scope. Reset to None whenever we hit a column-0 non-`func`
    /// line that isn't an indented continuation.
    current_func_line: Option<u32>,
}

/// Captured-but-not-yet-analyzed function declaration.
struct FuncStart {
    line_num: usize,
    name: String,
    params: String,
    param_names: Vec<String>,
    is_static: bool,
    return_type: Option<String>,
}

fn scan_line(line: &str, line_num: usize, state: &mut ScanState) {
    let trimmed = line.trim();
    let indented = line.starts_with('\t') || line.starts_with(' ');

    capture_extends(trimmed, &mut state.extends);
    capture_class_name(trimmed, &mut state.class_name);
    if !indented {
        capture_var_name(trimmed, &mut state.var_names);
    }
    capture_typed_decl(
        trimmed,
        scope_for_line(indented, state.current_func_line),
        &mut state.typed_decls,
    );

    if let Some(fs) = capture_func(trimmed, line_num, /* is_static */ true)
        .or_else(|| capture_func(trimmed, line_num, /* is_static */ false))
    {
        state.current_func_line = Some(u32::try_from(line_num + 1).unwrap_or(u32::MAX));
        push_typed_params(&fs.params, state.current_func_line, &mut state.typed_decls);
        state.func_starts.push(fs);
        return;
    }

    // A column-0 non-blank line that isn't a func / decorator closes
    // the open function's scope.
    if !indented && !trimmed.is_empty() && state.current_func_line.is_some() {
        state.current_func_line = None;
    }
}

fn capture_extends(trimmed: &str, slot: &mut Option<String>) {
    if slot.is_none()
        && let Some(cap) = EXTENDS.captures(trimmed)
        && let Some(m) = cap.get(1)
    {
        *slot = Some(m.as_str().to_owned());
    }
}

fn capture_class_name(trimmed: &str, slot: &mut Option<String>) {
    if slot.is_none()
        && let Some(cap) = CLASS_NAME.captures(trimmed)
        && let Some(m) = cap.get(1)
    {
        *slot = Some(m.as_str().to_owned());
    }
}

fn capture_var_name(trimmed: &str, out: &mut Vec<String>) {
    if let Some(cap) = VAR.captures(trimmed)
        && let Some(m) = cap.get(1)
    {
        out.push(m.as_str().to_owned());
    }
}

fn capture_typed_decl(trimmed: &str, scope: DeclScope, out: &mut Vec<TypedDecl>) {
    if let Some(cap) = TYPED_VAR.captures(trimmed)
        && let (Some(n), Some(t)) = (cap.get(1), cap.get(2))
    {
        out.push(TypedDecl {
            name: n.as_str().to_owned(),
            type_name: t.as_str().trim().to_owned(),
            scope,
        });
        return;
    }
    if let Some(cap) = INFERRED_NEW_VAR.captures(trimmed)
        && let (Some(n), Some(t)) = (cap.get(1), cap.get(2))
    {
        out.push(TypedDecl {
            name: n.as_str().to_owned(),
            type_name: t.as_str().to_owned(),
            scope,
        });
    }
}

fn capture_func(trimmed: &str, line_num: usize, is_static: bool) -> Option<FuncStart> {
    let re = if is_static { &*STATIC_FUNC } else { &*FUNC };
    let cap = re.captures(trimmed)?;
    let name = cap
        .get(1)
        .map(|m| m.as_str().to_owned())
        .unwrap_or_default();
    let params = cap
        .get(2)
        .map(|m| m.as_str().to_owned())
        .unwrap_or_default();
    let param_names = extract_param_names(&params);
    let return_type = cap.get(4).map(|m| m.as_str().trim().to_owned());
    Some(FuncStart {
        line_num,
        name,
        params,
        param_names,
        is_static,
        return_type,
    })
}

/// Pick the scope a `var` declaration belongs to.
///
/// Inside a tracked function body → function-scope; everywhere else →
/// module-scope. The "indented but no tracked function" edge case (rare;
/// usually a continuation line we don't model) falls into module-scope
/// as the safer default; over-scoping could let a local shadow a real
/// module var when the rewriter looks up receivers.
const fn scope_for_line(indented: bool, current_func_line: Option<u32>) -> DeclScope {
    if indented && let Some(func_line) = current_func_line {
        DeclScope::Function { func_line }
    } else {
        DeclScope::Module
    }
}

fn push_typed_params(params: &str, func_line: Option<u32>, out: &mut Vec<TypedDecl>) {
    let Some(line) = func_line else { return };
    for (name, type_name) in extract_typed_params(params) {
        out.push(TypedDecl {
            name,
            type_name,
            scope: DeclScope::Function { func_line: line },
        });
    }
}

#[cfg(test)]
mod tests;
