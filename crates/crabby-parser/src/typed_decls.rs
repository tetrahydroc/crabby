//! Typed declarations: variables and parameters whose declared type is
//! known at parse time.
//!
//! Used by the consumer-call rewriter to determine the type of a call's
//! receiver. With that type in hand the rewriter can decide whether
//! `<receiver>.<method>(...)` should route through an additive-template
//! hook variant (`_rtv_hooked_<method>`) or stay as the vanilla call.
//!
//! # What we track
//!
//! - **Module-scope** declarations: top-level `var x: Type` and
//!   `@export var x: Type`. Available everywhere in the script.
//! - **Function-scope** declarations: `func f(p: Type, ...)` parameters
//!   and `var x: Type` (or `var x := Type.new()`) inside the function
//!   body. Scoped to the function they appear in.
//!
//! # What we do NOT track (yet)
//!
//! - **Inferred types from arbitrary expressions.** `var x = some_call()`
//!   needs a real type inference pass; `x` is treated as untyped.
//!   This means some legitimate-receiver rewrites are missed (the
//!   consumer call falls through to the unwrapped vanilla method,
//!   skipping any mod hooks). That's a correctness loss for hooks but
//!   never a vanilla-behavior break.
//! - **Field-access chains.** `a.b.method()` would require knowing the
//!   type of `a.b`, which means walking field declarations across
//!   scripts. Out of scope for the first cut.
//! - **`if`-narrowed types** (`if x is Foo: x.method()`). Requires a
//!   per-branch scope, also out of scope.
//!
//! # Design notes
//!
//! Local-var detection uses indentation: anything inside a `func` body
//! starts with at least one tab or four spaces. Module-scope `var`s
//! start in column 0. The line scanner tracks the "current function" by
//! the most recent `func` line whose indent depth is shallower than the
//! current line; this is enough for the `GDScript` indent style actually
//! seen (one indent level inside func bodies, no nested funcs).

/// Lexical scope of a typed declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeclScope {
    /// Top-level: usable from any function body in the script.
    Module,
    /// Inside the function declared at `func_line` (1-based source line
    /// of the `func` keyword).
    Function {
        /// 1-based source line of the enclosing `func` declaration.
        func_line: u32,
    },
}

/// One typed declaration tied to a source location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedDecl {
    /// Variable / parameter name.
    pub name: String,
    /// Declared or inferred type as written. For `var x := Foo.new()`
    /// this is `"Foo"`. For `var x: Array[Foo]` this is `"Array[Foo]"`.
    pub type_name: String,
    /// Where this declaration is in scope.
    pub scope: DeclScope,
}
