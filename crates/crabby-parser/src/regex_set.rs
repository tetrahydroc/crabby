//! Lazily compiled regex set.
//!
//! Kept in one place so the patterns that define "what a declaration looks
//! like" live next to each other. `LazyLock` defers compilation to first
//! use, which is important because the rewriter will invoke `parse_script`
//! 150+ times per bake.

use std::sync::LazyLock;

use regex::Regex;

/// `^extends <Base>` or `^extends "<Base>"`.
pub static EXTENDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^extends\s+"?([\w/.:"]+)"?"#).expect("EXTENDS regex"));

/// `^class_name <Name>`.
pub static CLASS_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^class_name\s+(\w+)").expect("CLASS_NAME regex"));

/// `^func <name>(<params>)[ -> <return_type>]:`.
///
/// The return type capture widens vostok's `[\w\[\]]+` to also accept
/// `,`, `.`, and whitespace so that `Array[int]`, `Dictionary[String, int]`,
/// and dotted types like `Controller.State` parse correctly.
pub static FUNC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^func\s+(\w+)\s*\(([^)]*)\)(\s*->\s*([\w\[\],. ]+?))?\s*:").expect("FUNC regex")
});

/// `^static func <name>(<params>)[ -> <return_type>]:`.
pub static STATIC_FUNC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^static\s+func\s+(\w+)\s*\(([^)]*)\)(\s*->\s*([\w\[\],. ]+?))?\s*:")
        .expect("STATIC_FUNC regex")
});

/// `^[@export ]var <name>` for top-level variable declarations.
pub static VAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:@export\s+)?var\s+(\w+)").expect("VAR regex"));

/// `var <name>: <Type>` - explicitly typed variable. Matches anywhere on
/// the line so it works for both module-scope (no leading whitespace)
/// and function-scope (indented) declarations. The caller is responsible
/// for distinguishing scope by indentation.
///
/// Capture groups:
/// 1. variable name
/// 2. type as written (may include `Array[Foo]`, `Dictionary[K, V]`,
///    or dotted `Foo.Bar`)
pub static TYPED_VAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:@export\s+)?var\s+(\w+)\s*:\s*([\w\[\],. ]+?)\s*(?:=|$)")
        .expect("TYPED_VAR regex")
});

/// `var <name> := <Class>.new(` - type inferred from a `.new()` call on
/// a class name. Matches the simple form only; expression initializers
/// are intentionally not handled.
///
/// Capture groups:
/// 1. variable name
/// 2. inferred class name
pub static INFERRED_NEW_VAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"var\s+(\w+)\s*:?=\s*(\w+)\.new\s*\(").expect("INFERRED_NEW_VAR regex")
});
