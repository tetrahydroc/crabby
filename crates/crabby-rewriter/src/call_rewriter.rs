//! Consumer-side call-site rewriting for additive-template targets.
//!
//! After a resource-serialized script (e.g. `WorldSave.gd`) has been
//! rewritten with the [additive template](crate::template::additive),
//! its methods keep their original names. Dispatch wrappers are added
//! alongside under [`ADDITIVE_HOOK_PREFIX`](crate::resource_serialized::ADDITIVE_HOOK_PREFIX).
//!
//! For hooks to actually fire when consumer code calls into a
//! resource-serialized method, those consumer call sites must target the
//! wrapper, not the vanilla body. This module walks consumer source and
//! rewrites `<receiver>.<name>(...)` to `<receiver>._rtv_hooked_<name>(...)`,
//! but **only** when the receiver is provably one of the additive types.
//!
//! # Why type-aware
//!
//! Common method names like `Update`, `Reset`, `Save` collide between
//! additive scripts (`SlotData.Update`) and unrelated UI/utility classes
//! (`Tooltip.Update`). A blind rewriter routes `tooltip.Update(...)` to a
//! non-existent `tooltip._rtv_hooked_Update(...)`, breaking vanilla. The
//! parser's [`TypedDecl`](crabby_parser::TypedDecl) records are
//! consulted to decide per-call-site whether the receiver's type is in
//! the additive set.
//!
//! # What the rewriter does NOT touch
//!
//! - **Source inside `"..."` / `'...'` string literals**, the scanner
//!   tracks quote state and skips over them.
//! - **Full-line comments** (`#` in column 0 or after indentation).
//!   Trailing comments mid-line ARE skipped after the `#`.
//! - **Method *definitions*** (`func <name>(...):`), no leading `.`, so
//!   the pattern doesn't match.
//! - **Bare calls** like `save_data(x)`, no `.`, so no match.
//! - **Property access** `x.save_data` without parens, pattern requires
//!   `(` following the name.
//! - **Untyped receivers**, if the receiver type can't be proven, no
//!   rewrite. A missed hook is recoverable; a corrupted vanilla
//!   call site isn't.
//! - **Field-access chains** (`a.b.method()`), typing `a.b` requires
//!   walking field declarations across scripts; out of scope for this
//!   pass. Chains conservatively skip rewriting.
//!
//! # `self` and `super`
//!
//! `self.method(...)` typed-checks against the current script's class.
//! Since the consumer rewriter only runs on **non-additive** scripts,
//! `self` is never an additive type, so `self.<additive_method>(...)`
//! is never rewritten by this pass. (The additive script's own
//! intra-script `self.X` calls are emitted by the additive template
//! itself and route correctly.)
//!
//! `super.method(...)` is treated the same as `self`, it can't refer
//! to an additive type unless the current script extends one, which
//! none of the non-additive scripts in the corpus do. Skipped.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use crabby_parser::{DeclScope, ParsedScript, TypedDecl};

use crate::resource_serialized::ADDITIVE_HOOK_PREFIX;

/// Rewrite consumer call sites into their `_rtv_hooked_` variants.
///
/// Matches `<receiver>.<name>(` for every `name` in `additive_methods`
/// and rewrites to `<receiver>._rtv_hooked_<name>(`, **only** when
/// `<receiver>`'s declared type is one of `additive_types`.
///
/// `parsed` must come from parsing the same script `source` is derived
/// from (it carries the typed-decl table the rewriter consults). Pass
/// the original source's parsed script: the dispatch-wrapper injection
/// adds untyped scratch vars (`_lib`, `_repl`, ...) which are never
/// receivers of an additive method anyway, so missing them in the type
/// table doesn't cause incorrect rewrites.
///
/// `additive_types` should be the set of class names produced by
/// stripping `.gd` from each entry in
/// [`ADDITIVE_TEMPLATE_SCRIPTS`](crate::resource_serialized::ADDITIVE_TEMPLATE_SCRIPTS).
/// Accepted as a parameter rather than re-derived inside so the caller
/// can mock the set in tests.
// `HashSet<&str>` is taken with the default hasher rather than
// generalizing over `BuildHasher`: the set is built by this crate's own
// orchestrator with the default hasher and the extra generic churn
// across every callsite isn't worth the flexibility.
#[allow(clippy::implicit_hasher)]
#[must_use]
pub fn rewrite_consumer_calls(
    source: &str,
    parsed: &ParsedScript,
    additive_methods: &HashSet<&str>,
    additive_types: &HashSet<&str>,
) -> String {
    if additive_methods.is_empty() {
        return source.to_owned();
    }

    let scope = ScopeIndex::build(parsed);

    let mut out = String::with_capacity(source.len());
    // Mirror the parser's "current function" tracking to scope
    // receiver lookups. 1-based line numbers - `idx` is 0-based, so
    // `idx + 1` is passed into the scope index.
    let mut current_func_line: Option<u32> = None;
    for (idx, line) in source.split_inclusive('\n').enumerate() {
        let line_no = u32::try_from(idx + 1).unwrap_or(u32::MAX);
        let indented = line.starts_with('\t') || line.starts_with(' ');
        let trimmed = line.trim_start();

        // Update scope tracker. Any column-0 non-blank line that isn't
        // an indented continuation closes the previous function's scope;
        // a `func` at column 0 opens a new one.
        if !indented && !trimmed.is_empty() {
            if trimmed.starts_with("func ") || trimmed.starts_with("static func ") {
                current_func_line = Some(line_no);
            } else {
                current_func_line = None;
            }
        }

        rewrite_line(
            line,
            additive_methods,
            additive_types,
            &scope,
            current_func_line,
            &mut out,
        );
    }
    out
}

/// Rewrite one line into `out`, skipping string-literal and comment
/// regions. Preserves the trailing newline (if any) verbatim.
///
/// # UTF-8 handling
///
/// The scanner reads bytes for speed and token-boundary simplicity
/// (identifiers, quotes, `.` are all single-byte ASCII), but every time
/// it *emits* characters, it emits whole UTF-8 sequences copied from
/// `line` via `str::push_str`. Naive `push(b as char)` would treat each
/// UTF-8 continuation byte as its own codepoint and re-encode it,
/// producing double-encoded output (e.g. `€` -> `â‚¬`). Byte-level
/// decisions (quote state, `.`-prefix detection) only ever trigger on
/// ASCII bytes, so the byte/char granularity mismatch is safe as long
/// as non-ASCII emission goes through `emit_char_at`.
fn rewrite_line(
    line: &str,
    methods: &HashSet<&str>,
    types: &HashSet<&str>,
    scope: &ScopeIndex,
    current_func_line: Option<u32>,
    out: &mut String,
) {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut quote: Option<u8> = None;

    while i < bytes.len() {
        let b = bytes[i];

        if quote.is_none() && b == b'#' {
            out.push_str(&line[i..]);
            return;
        }

        if let Some(q) = quote {
            let step = emit_char_at(line, i, out);
            if b == b'\\' && i + step < bytes.len() {
                i += step;
                let esc_step = emit_char_at(line, i, out);
                i += esc_step;
                continue;
            }
            if b == q {
                quote = None;
            }
            i += step;
            continue;
        }

        if b == b'"' || b == b'\'' {
            quote = Some(b);
            out.push(b as char);
            i += 1;
            continue;
        }

        if b == b'.'
            && let Some(consumed) = try_rewrite_method_call(
                line.as_bytes(),
                i,
                methods,
                types,
                scope,
                current_func_line,
                out,
            )
        {
            i += consumed;
            continue;
        }

        i += emit_char_at(line, i, out);
    }
}

/// Emit the UTF-8 character whose first byte is at `bytes[i]` into
/// `out`, preserving the original encoding. Returns the number of bytes
/// consumed (1 for ASCII, 2-4 for multi-byte sequences).
fn emit_char_at(line: &str, i: usize, out: &mut String) -> usize {
    let bytes = line.as_bytes();
    let b = bytes[i];
    if b < 0x80 {
        out.push(b as char);
        return 1;
    }
    let len = utf8_char_len(b);
    // Guard against malformed input: if len runs past the end, copy
    // the remainder as a best-effort through a from_utf8 round-trip.
    // Real GDSC source never produces malformed UTF-8 so this branch is
    // defensive only.
    let end = (i + len).min(bytes.len());
    out.push_str(&line[i..end]);
    end - i
}

const fn utf8_char_len(first_byte: u8) -> usize {
    match first_byte {
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        // ASCII (0x00..=0x7F) handled by the caller's early return;
        // continuation bytes (0x80..=0xBF, 0xF8..) are malformed first
        // bytes so advance one byte and move on. Real GDSC source
        // never produces those.
        _ => 1,
    }
}

/// Try to consume a `<receiver>.<name>(...)` pattern where the `.` is at
/// `bytes[dot]`. On match, push the rewritten form into `out` and return
/// the number of input bytes consumed *starting from the `.`* (so the
/// outer loop can advance `i` correctly). On no match, return `None`.
#[allow(clippy::too_many_arguments)]
fn try_rewrite_method_call(
    bytes: &[u8],
    dot: usize,
    methods: &HashSet<&str>,
    types: &HashSet<&str>,
    scope: &ScopeIndex,
    current_func_line: Option<u32>,
    out: &mut String,
) -> Option<usize> {
    debug_assert_eq!(bytes[dot], b'.');

    // `..` and `...` are range operators, not property access.
    if bytes.get(dot + 1) == Some(&b'.') {
        return None;
    }

    // Identifier immediately after the `.`.
    let name_start = dot + 1;
    let mut name_end = name_start;
    while name_end < bytes.len() && is_ident_byte(bytes[name_end]) {
        name_end += 1;
    }
    if name_end == name_start {
        return None;
    }
    let name = std::str::from_utf8(&bytes[name_start..name_end]).ok()?;
    if !methods.contains(name) {
        return None;
    }

    // Whitespace then `(` (it's a call, not property access).
    let mut scan = name_end;
    while scan < bytes.len() && (bytes[scan] == b' ' || bytes[scan] == b'\t') {
        scan += 1;
    }
    if scan >= bytes.len() || bytes[scan] != b'(' {
        return None;
    }

    // Receiver type check: scan backwards from `dot` to extract the
    // receiver token. Only rewrite when the receiver is provably an
    // additive type.
    let rty = receiver_type_for_call(bytes, dot, scope, current_func_line)?;
    if !types.contains(rty) {
        return None;
    }

    let _ = write!(out, ".{ADDITIVE_HOOK_PREFIX}{name}");
    out.push_str(
        std::str::from_utf8(&bytes[name_end..scan]).expect("ASCII whitespace is valid UTF-8"),
    );
    out.push('(');
    Some(scan + 1 - dot)
}

/// Determine the receiver's type for a call whose `.` is at `bytes[dot]`.
///
/// Returns `Some(type_name)` only when the receiver is a simple
/// identifier present in the scope index. Chains
/// (`a.b.method`), call results (`f().method`), index results
/// (`arr[0].method`), and typeless identifiers all return `None`,
/// conservative skip.
fn receiver_type_for_call<'s>(
    bytes: &[u8],
    dot: usize,
    scope: &'s ScopeIndex,
    current_func_line: Option<u32>,
) -> Option<&'s str> {
    // Scan back to find the start of the receiver identifier.
    if dot == 0 {
        return None;
    }
    let mut start = dot;
    while start > 0 && is_ident_byte(bytes[start - 1]) {
        start -= 1;
    }
    if start == dot {
        return None; // no identifier immediately before the `.`
    }
    // Reject chains / call results / index results: char before the
    // receiver should NOT be `.`, `)`, or `]`.
    if start > 0 {
        let prev = bytes[start - 1];
        if prev == b'.' || prev == b')' || prev == b']' {
            return None;
        }
    }
    let receiver = std::str::from_utf8(&bytes[start..dot]).ok()?;

    // `self` and `super` carry the current script's type; the consumer
    // rewriter only runs on non-additive scripts, so these can never
    // resolve to an additive class. Skip without lookup.
    if receiver == "self" || receiver == "super" {
        return None;
    }

    scope.lookup(receiver, current_func_line)
}

/// Indexed view over a script's typed declarations for fast receiver
/// lookups by name + scope.
///
/// Function-scope decls shadow module-scope decls of the same name,
/// matching `GDScript`'s lexical resolution.
struct ScopeIndex<'a> {
    /// Module-scope: name -> type.
    module: HashMap<&'a str, &'a str>,
    /// Function-scope: (`func_line`, name) -> type.
    function: HashMap<(u32, &'a str), &'a str>,
}

impl<'a> ScopeIndex<'a> {
    fn build(parsed: &'a ParsedScript) -> Self {
        let mut module = HashMap::new();
        let mut function = HashMap::new();
        for d in &parsed.typed_decls {
            insert_decl(d, &mut module, &mut function);
        }
        Self { module, function }
    }

    fn lookup(&self, name: &str, current_func_line: Option<u32>) -> Option<&'a str> {
        if let Some(line) = current_func_line
            && let Some(t) = self.function.get(&(line, name))
        {
            return Some(*t);
        }
        self.module.get(name).copied()
    }
}

fn insert_decl<'a>(
    d: &'a TypedDecl,
    module: &mut HashMap<&'a str, &'a str>,
    function: &mut HashMap<(u32, &'a str), &'a str>,
) {
    match d.scope {
        DeclScope::Module => {
            module.insert(d.name.as_str(), d.type_name.as_str());
        }
        DeclScope::Function { func_line } => {
            function.insert((func_line, d.name.as_str()), d.type_name.as_str());
        }
    }
}

const fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crabby_parser::parse_script;

    fn methods(names: &[&'static str]) -> HashSet<&'static str> {
        names.iter().copied().collect()
    }

    fn types(names: &[&'static str]) -> HashSet<&'static str> {
        names.iter().copied().collect()
    }

    fn rewrite(
        src: &str,
        additive_methods: &[&'static str],
        additive_types: &[&'static str],
    ) -> String {
        let parsed = parse_script("Caller.gd", src).unwrap();
        rewrite_consumer_calls(
            src,
            &parsed,
            &methods(additive_methods),
            &types(additive_types),
        )
    }

    #[test]
    fn empty_method_set_is_noop() {
        let src = "var x: SlotData\nfunc f():\n\tx.save_data(y)\n";
        let out = rewrite(src, &[], &["SlotData"]);
        assert_eq!(out, src);
    }

    #[test]
    fn rewrites_typed_module_var() {
        let src = "\
extends Node

@export var slot: SlotData

func f():
\tslot.save_data(dict)
";
        let out = rewrite(src, &["save_data"], &["SlotData"]);
        assert!(out.contains("slot._rtv_hooked_save_data(dict)"), "{out}");
    }

    #[test]
    fn rewrites_typed_function_param() {
        let src = "\
extends Node

func f(s: SlotData):
\ts.save_data(dict)
";
        let out = rewrite(src, &["save_data"], &["SlotData"]);
        assert!(out.contains("s._rtv_hooked_save_data(dict)"), "{out}");
    }

    #[test]
    fn rewrites_typed_local_var() {
        let src = "\
extends Node

func f():
\tvar s: SlotData = make()
\ts.save_data(dict)
";
        let out = rewrite(src, &["save_data"], &["SlotData"]);
        assert!(out.contains("s._rtv_hooked_save_data(dict)"), "{out}");
    }

    #[test]
    fn rewrites_inferred_new() {
        let src = "\
extends Node

func f():
\tvar s := SlotData.new()
\ts.save_data(dict)
";
        let out = rewrite(src, &["save_data"], &["SlotData"]);
        assert!(out.contains("s._rtv_hooked_save_data(dict)"), "{out}");
    }

    #[test]
    fn does_not_rewrite_untyped_receiver() {
        // `tooltip` has no typed declaration -> conservative skip even
        // though the method name matches an additive method.
        let src = "\
extends Node

func f():
\ttooltip.Update(item)
";
        let out = rewrite(src, &["Update"], &["SlotData"]);
        assert_eq!(out, src);
    }

    #[test]
    fn does_not_rewrite_wrong_typed_receiver() {
        // `tooltip` is typed Tooltip (not in additive set) -> skip.
        let src = "\
extends Node

@export var tooltip: Tooltip

func f():
\ttooltip.Update(item)
";
        let out = rewrite(src, &["Update"], &["SlotData"]);
        assert_eq!(out, src);
    }

    #[test]
    fn does_not_rewrite_chain_receiver() {
        // `newItem.slotData` is a field-access chain. Typing
        // `newItem.slotData` requires cross-script field resolution, so
        // no rewrite. (A hook on `SlotData.Update` won't fire when
        // called via `newItem.slotData.Update(x)`, documented limitation.)
        let src = "\
extends Node

@export var newItem: Node

func f():
\tnewItem.slotData.Update(x)
";
        let out = rewrite(src, &["Update"], &["SlotData"]);
        assert_eq!(out, src);
    }

    #[test]
    fn does_not_rewrite_self() {
        // `self` is the consumer script's own class, never an additive
        // type when this rewriter runs (orchestration only invokes it on
        // non-additive scripts).
        let src = "\
extends Node

func f():
\tself.Update(x)
";
        let out = rewrite(src, &["Update"], &["SlotData"]);
        assert_eq!(out, src);
    }

    #[test]
    fn does_not_rewrite_property_access_without_parens() {
        let src = "\
extends Node

@export var s: SlotData

func f():
\tvar x = s.save_data
";
        let out = rewrite(src, &["save_data"], &["SlotData"]);
        assert_eq!(out, src);
    }

    #[test]
    fn does_not_rewrite_inside_strings() {
        let src = "\
extends Node

@export var s: SlotData

func f():
\tvar lit = \"s.save_data(x)\"
\ts.save_data(real)
";
        let out = rewrite(src, &["save_data"], &["SlotData"]);
        assert!(out.contains("\"s.save_data(x)\""));
        assert!(out.contains("s._rtv_hooked_save_data(real)"));
    }

    #[test]
    fn does_not_rewrite_inside_comments() {
        let src = "\
extends Node

@export var s: SlotData

func f():
\t# s.save_data(x)
\ts.save_data(y)
";
        let out = rewrite(src, &["save_data"], &["SlotData"]);
        assert!(out.contains("# s.save_data(x)"));
        assert!(out.contains("s._rtv_hooked_save_data(y)"));
    }

    #[test]
    fn function_scope_shadows_module_scope() {
        // Module `s: Tooltip`, but inside `f` a local `s: SlotData`
        // shadows it. The local wins for calls inside `f`.
        let src = "\
extends Node

@export var s: Tooltip

func f():
\tvar s: SlotData = make()
\ts.Update(x)
";
        let out = rewrite(src, &["Update"], &["SlotData"]);
        assert!(out.contains("s._rtv_hooked_Update(x)"), "{out}");
    }

    #[test]
    fn function_scope_isolated_to_its_function() {
        // `s: SlotData` declared in `f`, but `g` uses module-scope `s`.
        let src = "\
extends Node

@export var s: Tooltip

func f():
\tvar s: SlotData = make()
\ts.Update(x)

func g():
\ts.Update(y)
";
        let out = rewrite(src, &["Update"], &["SlotData"]);
        // f's call rewrites; g's does not (s is Tooltip there).
        let f_idx = out.find("func f():").unwrap();
        let g_idx = out.find("func g():").unwrap();
        assert!(out[f_idx..g_idx].contains("s._rtv_hooked_Update(x)"));
        assert!(out[g_idx..].contains("s.Update(y)"));
        assert!(!out[g_idx..].contains("_rtv_hooked_Update"));
    }

    #[test]
    fn range_operators_pass_through() {
        let src = "\
extends Node

func f():
\tvar slice = arr[0..5]
";
        let out = rewrite(src, &["save_data"], &["SlotData"]);
        assert_eq!(out, src);
    }

    #[test]
    fn word_boundary_on_method_name_prefix() {
        // `s.save_data_extra(x)` must NOT match when target is `save_data`.
        let src = "\
extends Node

@export var s: SlotData

func f():
\ts.save_data_extra(x)
";
        let out = rewrite(src, &["save_data"], &["SlotData"]);
        assert_eq!(out, src);
    }

    #[test]
    fn multiple_calls_same_line_typed() {
        let src = "\
extends Node

@export var a: SlotData
@export var b: SlotData

func f():
\ta.save_data(x); b.save_data(y)
";
        let out = rewrite(src, &["save_data"], &["SlotData"]);
        assert!(out.contains("a._rtv_hooked_save_data(x); b._rtv_hooked_save_data(y)"));
    }

    #[test]
    fn preserves_whitespace_between_name_and_paren() {
        let src = "\
extends Node

@export var s: SlotData

func f():
\ts.save_data  \t(x)
";
        let out = rewrite(src, &["save_data"], &["SlotData"]);
        assert!(out.contains("s._rtv_hooked_save_data  \t(x)"));
    }

    #[test]
    fn preserves_utf8_string_literals() {
        // Non-ASCII characters inside string literals must round-trip
        // byte-identical. A naive byte-by-byte copy via `char::from(b)`
        // double-encodes UTF-8 (€ -> â‚¬). This test catches that.
        let src = "\
extends Node

@export var s: SlotData

func f():
\tvar text = \"Value: \u{20ac}\"
\ts.save_data(text)
";
        let out = rewrite(src, &["save_data"], &["SlotData"]);
        // Literal must stay as the single € character, not 3 mojibake chars.
        assert!(
            out.contains("\"Value: \u{20ac}\""),
            "UTF-8 got mangled; output bytes:\n{:?}",
            out.as_bytes(),
        );
        // And the call-site rewrite still fires.
        assert!(out.contains("s._rtv_hooked_save_data(text)"));
    }

    #[test]
    fn preserves_utf8_outside_strings() {
        // UTF-8 can appear in comments (trailing) and in identifiers
        // (Godot allows Unicode-letter identifiers, though vanilla RTV
        // doesn't use them). Copy through verbatim.
        let src = "\
extends Node

@export var s: SlotData

func f():
\ts.save_data(x) # ümlaut comment
";
        let out = rewrite(src, &["save_data"], &["SlotData"]);
        assert!(out.contains("# ümlaut comment"), "{out}");
        assert!(out.contains("s._rtv_hooked_save_data(x)"));
    }
}
