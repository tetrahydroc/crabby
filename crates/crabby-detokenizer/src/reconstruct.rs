//! Reconstruct source text from a [`ParsedScript`](crate::decode::ParsedScript).
//!
//! This is the port of vostok's `_gdsc_reconstruct`. Indentation is emitted
//! as tabs (`col / 4` tabs per token that opens a new line). Spacing logic
//! mirrors vostok's `_SPACE_BEFORE` / `_SPACE_AFTER` tables, kept in
//! [`tokens`](crate::tokens) for testability.

use std::collections::HashMap;

use crate::decode::{ParsedScript, Variant};
use crate::tokens::{
    FIRST_CONTROL_FLOW_KW, FIRST_KEYWORD, LAST_CONTROL_FLOW_KW, LAST_KEYWORD, RawToken,
    TK_ANNOTATION, TK_BANG, TK_BRACE_CLOSE, TK_BRACKET_CLOSE, TK_BRACKET_OPEN, TK_DEDENT,
    TK_DOLLAR, TK_DOT, TK_EOF, TK_IDENTIFIER, TK_INDENT, TK_INF, TK_LITERAL, TK_NAN, TK_NEWLINE,
    TK_NOT_WORD, TK_PAREN_CLOSE, TK_PAREN_OPEN, TK_PI, TK_TAU, TK_TILDE, TK_UNDERSCORE,
    space_after, space_before, token_text,
};

/// Reconstruct source text from a parsed script.
#[must_use]
pub fn emit(parsed: &ParsedScript) -> String {
    let line_lookup: HashMap<u32, u32> = parsed.line_map.iter().copied().collect();
    let col_lookup: HashMap<u32, u32> = parsed.col_map.iter().copied().collect();

    let mut state = EmitState::fresh();

    for (i, token) in parsed.tokens.iter().enumerate() {
        let i_u32 = u32::try_from(i).unwrap_or(u32::MAX);

        // Advance line counter if the line map puts this token past the current line.
        if let Some(&target_line) = line_lookup.get(&i_u32) {
            while state.current_line_num < target_line {
                state.push_current_line();
            }
        }

        if token.kind == TK_EOF {
            break;
        }
        if token.kind == TK_NEWLINE {
            state.push_current_line();
            state.prev_kind = Some(token.kind);
            continue;
        }
        if token.kind == TK_INDENT || token.kind == TK_DEDENT {
            state.prev_kind = Some(token.kind);
            continue;
        }

        let text = render_token(*token, parsed);
        emit_token(
            &mut state,
            token.kind,
            &text,
            col_lookup.get(&i_u32).copied(),
        );
    }

    state.finalize()
}

struct EmitState {
    lines: Vec<String>,
    current_line: String,
    /// Tracks source-line position so we can emit blank lines when the line
    /// map jumps ahead. 1-based to match Godot's line numbering.
    current_line_num: u32,
    need_space: bool,
    prev_kind: Option<u32>,
    line_started: bool,
}

impl EmitState {
    const fn fresh() -> Self {
        Self {
            lines: Vec::new(),
            current_line: String::new(),
            current_line_num: 1,
            need_space: false,
            prev_kind: None,
            line_started: false,
        }
    }

    fn push_current_line(&mut self) {
        let line = std::mem::take(&mut self.current_line);
        self.lines.push(line);
        self.current_line_num += 1;
        self.need_space = false;
        self.line_started = false;
    }

    fn finalize(mut self) -> String {
        if !self.current_line.is_empty() {
            self.lines.push(self.current_line);
        }
        let mut out = self.lines.join("\n");
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out
    }
}

fn emit_token(state: &mut EmitState, kind: u32, text: &str, col: Option<u32>) {
    // Indent on first visible token of the line.
    if !state.line_started {
        state.line_started = true;
        if let Some(c) = col {
            let tabs = c / 4;
            for _ in 0..tabs {
                state.current_line.push('\t');
            }
        }
    }

    let wants_space_before = needs_space_before(state, kind);
    if wants_space_before
        && !state.current_line.is_empty()
        && !state.current_line.ends_with(' ')
        && !state.current_line.ends_with('\t')
    {
        state.current_line.push(' ');
    }

    state.current_line.push_str(text);
    state.need_space = needs_space_after(kind);
    state.prev_kind = Some(kind);
}

fn needs_space_before(state: &EmitState, kind: u32) -> bool {
    if !state.need_space || state.current_line.is_empty() || state.current_line.ends_with('\t') {
        return false;
    }
    if space_before(kind) {
        return true;
    }
    let prev = state.prev_kind;

    if is_ident_or_literal(kind) || (FIRST_KEYWORD..=LAST_KEYWORD).contains(&kind) {
        let skip_annotation =
            matches!(prev, Some(TK_ANNOTATION)) && (kind == TK_IDENTIFIER || kind == TK_ANNOTATION);
        if skip_annotation {
            return false;
        }
        return !matches!(
            prev,
            Some(
                TK_PAREN_OPEN
                    | TK_BRACKET_OPEN
                    | TK_DOT
                    | TK_DOLLAR
                    | TK_TILDE
                    | TK_BANG
                    | TK_INDENT
                    | TK_NEWLINE,
            ) | None,
        );
    }

    if kind == TK_PAREN_OPEN {
        return matches!(prev, Some(k) if (FIRST_CONTROL_FLOW_KW..=LAST_CONTROL_FLOW_KW).contains(&k));
    }

    if kind == TK_NOT_WORD || kind == TK_BANG {
        return true;
    }

    false
}

const fn is_ident_or_literal(kind: u32) -> bool {
    matches!(kind, TK_IDENTIFIER | TK_LITERAL | TK_ANNOTATION)
}

const fn needs_space_after(kind: u32) -> bool {
    space_after(kind)
        || matches!(
            kind,
            TK_IDENTIFIER
                | TK_LITERAL
                | TK_PAREN_CLOSE
                | TK_BRACKET_CLOSE
                | TK_BRACE_CLOSE
                | TK_PI
                | TK_TAU
                | TK_INF
                | TK_NAN
                | TK_UNDERSCORE,
        )
}

fn render_token(token: RawToken, parsed: &ParsedScript) -> String {
    let idx = token.data_index as usize;
    match token.kind {
        TK_IDENTIFIER => parsed
            .identifiers
            .get(idx)
            .cloned()
            .unwrap_or_else(|| "<ident?>".into()),
        TK_ANNOTATION => {
            let name = parsed
                .identifiers
                .get(idx)
                .cloned()
                .unwrap_or_else(|| "?".into());
            if name.starts_with('@') {
                name
            } else {
                format!("@{name}")
            }
        }
        TK_LITERAL => parsed
            .constants
            .get(idx)
            .map_or_else(|| "null".into(), variant_to_source),
        _ => {
            token_text(token.kind).map_or_else(|| format!("<tk{}>", token.kind), ToOwned::to_owned)
        }
    }
}

/// Convert a [`Variant`] back to a `GDScript` literal form. Port of vostok's
/// `_gdsc_variant_to_source`.
fn variant_to_source(v: &Variant) -> String {
    match v {
        Variant::Nil => "null".into(),
        Variant::Bool(true) => "true".into(),
        Variant::Bool(false) => "false".into(),
        Variant::Int(n) => n.to_string(),
        Variant::Float(f) => format_float(*f),
        Variant::String(s) => format!("\"{}\"", escape(s)),
        Variant::StringName(s) => format!("&\"{}\"", escape(s)),
        Variant::NodePath(s) => format!("^\"{}\"", escape(s)),
    }
}

/// Match `GDScript`'s `str(float)` output: finite floats without a decimal
/// point get `.0` appended so they round-trip as `float` rather than `int`.
fn format_float(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f.is_sign_negative() {
            "-inf".into()
        } else {
            "inf".into()
        };
    }
    let s = format!("{f}");
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{s}.0")
    }
}

/// Minimal `GDScript` string-escape: backslash, double-quote, newline, tab, carriage return, null.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_float_adds_trailing_zero_when_missing() {
        assert_eq!(format_float(1.0), "1.0");
        assert_eq!(format_float(42.0), "42.0");
    }

    #[test]
    fn format_float_preserves_fractional() {
        assert_eq!(format_float(1.5), "1.5");
    }

    #[test]
    fn format_float_handles_special_values() {
        assert_eq!(format_float(f64::INFINITY), "inf");
        assert_eq!(format_float(f64::NEG_INFINITY), "-inf");
        assert_eq!(format_float(f64::NAN), "nan");
    }

    #[test]
    fn escape_handles_common_cases() {
        assert_eq!(escape("plain"), "plain");
        assert_eq!(escape("quo\"te"), "quo\\\"te");
        assert_eq!(escape("back\\slash"), "back\\\\slash");
        assert_eq!(escape("line\nfeed"), "line\\nfeed");
    }

    #[test]
    fn variant_to_source_covers_known_types() {
        assert_eq!(variant_to_source(&Variant::Nil), "null");
        assert_eq!(variant_to_source(&Variant::Bool(true)), "true");
        assert_eq!(variant_to_source(&Variant::Int(-7)), "-7");
        assert_eq!(variant_to_source(&Variant::Float(2.5)), "2.5");
        assert_eq!(variant_to_source(&Variant::String("hi".into())), "\"hi\"");
        assert_eq!(
            variant_to_source(&Variant::StringName("hi".into())),
            "&\"hi\"",
        );
        assert_eq!(
            variant_to_source(&Variant::NodePath("/Root/Child".into())),
            "^\"/Root/Child\"",
        );
    }
}
