//! Engine-lifecycle methods that are always void, regardless of body.
//!
//! Matches vostok-mod-loader's `RTV_ENGINE_VOID_METHODS` list in
//! `constants.gd`. Keep the two in sync, a drift here would force crabby
//! into the non-void template for `_ready` et al., which breaks dispatch
//! semantics (void templates skip the `_result` tracking).

/// Engine lifecycle methods known to return no value. The rewriter picks
/// the void template for these even if the body contains a `return <expr>`
/// (which would imply non-void via body analysis alone).
pub const ENGINE_VOID_METHODS: &[&str] = &[
    "_ready",
    "_process",
    "_physics_process",
    "_input",
    "_unhandled_input",
    "_unhandled_key_input",
    "_enter_tree",
    "_exit_tree",
    "_notification",
];

/// Whether `name` is one of the fixed engine-lifecycle methods that the
/// rewriter must treat as void regardless of body content.
#[must_use]
pub fn is_engine_void_method(name: &str) -> bool {
    ENGINE_VOID_METHODS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_and_process_are_void() {
        assert!(is_engine_void_method("_ready"));
        assert!(is_engine_void_method("_process"));
        assert!(is_engine_void_method("_physics_process"));
    }

    #[test]
    fn regular_methods_are_not() {
        assert!(!is_engine_void_method("ApplyDamage"));
        assert!(!is_engine_void_method("_physicsProcess")); // different casing
    }
}
