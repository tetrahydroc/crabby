//! Parameter-list tokenization.
//!
//! Given the raw parameter string between `(` and `)`, extract the parameter
//! names (dropping types, defaults). Port of vostok's `_rtv_extract_param_names`.
//!
//! Splitting is naive (comma-separated) - it matches vostok's behavior.
//! Vanilla RTV doesn't use default values containing commas; if a mod did
//! (e.g. `func foo(arr := [1, 2])`), the param count would be wrong, but
//! the rewriter only needs names to build call-through signatures where
//! comma placement doesn't matter.

/// Extract parameter names from a raw params string like
/// `"x: int, y := 2, z: String = \"hi\""` → `["x", "y", "z"]`.
#[must_use]
pub fn extract_param_names(params: &str) -> Vec<String> {
    let trimmed = params.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    trimmed
        .split(',')
        .filter_map(|p| {
            // Drop type annotation and default value.
            // "x: int = 1" → "x: int " → "x "  → "x"
            let without_type = p.split(':').next()?;
            let without_default = without_type.split('=').next()?;
            let name = without_default.trim();
            if name.is_empty() {
                None
            } else {
                Some(name.to_owned())
            }
        })
        .collect()
}

/// Extract `(name, type)` pairs for parameters that have an explicit
/// type annotation (`name: Type`).
///
/// Untyped parameters and walrus-inferred parameters (`name := default`)
/// are skipped because we don't have a type to record. The caller can
/// still get all names via [`extract_param_names`] when it needs them
/// for the call-through signature.
///
/// Type strings are returned as written, with bracketed generics
/// preserved (`Array[Foo]`, `Dictionary[K, V]`).
#[must_use]
pub fn extract_typed_params(params: &str) -> Vec<(String, String)> {
    let trimmed = params.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for raw in split_top_level_commas(trimmed) {
        let p = raw.trim();
        if p.is_empty() {
            continue;
        }
        // Look for `name : Type` (with optional `= default` after). The
        // walrus `:=` form is excluded because it's `name := value` with
        // no explicit type.
        let Some(colon) = find_type_colon(p) else {
            continue;
        };
        let name = p[..colon].trim();
        if name.is_empty() {
            continue;
        }
        let after = p[colon + 1..].trim();
        // Strip default value: take everything before the first top-level `=`.
        let type_part = after.split('=').next().unwrap_or(after).trim();
        if type_part.is_empty() {
            continue;
        }
        out.push((name.to_owned(), type_part.to_owned()));
    }
    out
}

/// Split on `,` but only at depth 0; keeps `Array[A, B]` together.
fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth: i32 = 0;
    let mut start = 0;
    for (i, b) in s.bytes().enumerate() {
        match b {
            b'[' | b'(' | b'{' => depth += 1,
            b']' | b')' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// Find the `:` that introduces a type annotation, ignoring `:=`
/// (walrus). Returns None if no type-introducing colon exists.
fn find_type_colon(p: &str) -> Option<usize> {
    let bytes = p.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b':' {
            // Skip walrus `:=`, that's an inference operator, not a
            // type annotation.
            if bytes.get(i + 1) == Some(&b'=') {
                return None;
            }
            return Some(i);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_params() {
        assert_eq!(extract_param_names(""), Vec::<String>::new());
        assert_eq!(extract_param_names("   "), Vec::<String>::new());
    }

    #[test]
    fn untyped_params() {
        assert_eq!(extract_param_names("a"), vec!["a"]);
        assert_eq!(extract_param_names("a, b, c"), vec!["a", "b", "c"]);
    }

    #[test]
    fn typed_params() {
        assert_eq!(extract_param_names("x: int, y: String"), vec!["x", "y"]);
    }

    #[test]
    fn defaults_stripped() {
        assert_eq!(
            extract_param_names("x: int = 1, y := 2, z: String = \"hi\""),
            vec!["x", "y", "z"],
        );
    }

    #[test]
    fn inferred_type_defaults() {
        // `:=` walrus-like inferred type + default. First `:` stripping
        // happens to drop `= 2` too, leaving just "x"; extract_param_names
        // returns "x".
        assert_eq!(extract_param_names("x := 2"), vec!["x"]);
    }

    #[test]
    fn whitespace_tolerated() {
        assert_eq!(
            extract_param_names("   a   ,   b:int  ,  c=1  "),
            vec!["a", "b", "c"],
        );
    }

    #[test]
    fn extract_typed_params_basic() {
        let got = extract_typed_params("x: int, y: SlotData, z: String");
        assert_eq!(
            got,
            vec![
                ("x".to_owned(), "int".to_owned()),
                ("y".to_owned(), "SlotData".to_owned()),
                ("z".to_owned(), "String".to_owned()),
            ],
        );
    }

    #[test]
    fn extract_typed_params_skips_untyped() {
        // Untyped + walrus-inferred should both be dropped, no
        // explicit type to record.
        let got = extract_typed_params("a, b := 2, c: SlotData");
        assert_eq!(got, vec![("c".to_owned(), "SlotData".to_owned())]);
    }

    #[test]
    fn extract_typed_params_with_defaults() {
        let got = extract_typed_params("x: int = 1, y: String = \"hi\"");
        assert_eq!(
            got,
            vec![
                ("x".to_owned(), "int".to_owned()),
                ("y".to_owned(), "String".to_owned()),
            ],
        );
    }

    #[test]
    fn extract_typed_params_array_generics() {
        let got = extract_typed_params("items: Array[ItemData], map: Dictionary[String, int]");
        assert_eq!(
            got,
            vec![
                ("items".to_owned(), "Array[ItemData]".to_owned()),
                ("map".to_owned(), "Dictionary[String, int]".to_owned()),
            ],
        );
    }

    #[test]
    fn extract_typed_params_empty() {
        assert!(extract_typed_params("").is_empty());
        assert!(extract_typed_params("   ").is_empty());
    }
}
