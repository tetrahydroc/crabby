//! Hook-name formatting.
//!
//! Crabby mirrors vostok-mod-loader's hook naming exactly so mods written
//! for either loader address the same hooks:
//!
//! - script prefix = filename stem, lowercased (e.g. `Controller.gd` ->
//!   `controller`)
//! - hook base     = `<script>-<method>`, both lowercased (e.g.
//!   `controller-_physics_process`)
//! - hook variants = `<base>-pre`, `<base>-post`, `<base>-callback`, plus
//!   the bare `<base>` for replace hooks

/// Derive the script prefix from a filename.
///
/// ```
/// use crabby_rewriter::script_prefix;
/// assert_eq!(script_prefix("Controller.gd"), "controller");
/// assert_eq!(script_prefix("AISpawner.gd"), "aispawner");
/// ```
#[must_use]
pub fn script_prefix(filename: &str) -> String {
    let stem = filename.strip_suffix(".gd").unwrap_or(filename);
    stem.to_ascii_lowercase()
}

/// Construct the hook base from a script prefix and a method name.
///
/// ```
/// use crabby_rewriter::hook_base;
/// assert_eq!(hook_base("controller", "_physics_process"), "controller-_physics_process");
/// assert_eq!(hook_base("hitbox", "ApplyDamage"), "hitbox-applydamage");
/// ```
#[must_use]
pub fn hook_base(prefix: &str, method: &str) -> String {
    format!("{prefix}-{}", method.to_ascii_lowercase())
}

/// Per-base flags recording which hook KINDS are actually registered
/// against a hook BASE name across all enabled mods. Drives the
/// per-kind dispatch-site elision in the wrapper templates.
///
/// Defined here (not in the analyzer) so the rewriter and its templates
/// stay free of analyzer dependencies; the analyzer converts its own
/// `HookKindsPresent` into `HookFlags` when building the bake input.
///
/// `Default` is "no kinds set", equivalent to a base that nobody hooks
/// (the wrapper would be skipped entirely under AOT skip).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HookFlags {
    /// At least one `-pre` hook is registered for this base.
    pub pre: bool,
    /// At least one `-post` hook is registered.
    pub post: bool,
    /// At least one `-callback` hook is registered.
    pub callback: bool,
    /// At least one bare-name (replace) hook is registered.
    pub replace: bool,
}

impl HookFlags {
    /// Convenience constructor: every kind set. Matches the legacy
    /// "wrap everything" emission.
    #[must_use]
    pub fn all() -> Self {
        Self { pre: true, post: true, callback: true, replace: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_strips_gd_suffix_and_lowercases() {
        assert_eq!(script_prefix("Camera.gd"), "camera");
        assert_eq!(script_prefix("LootContainer.gd"), "lootcontainer");
    }

    #[test]
    fn prefix_handles_missing_extension() {
        assert_eq!(script_prefix("Camera"), "camera");
    }

    #[test]
    fn base_preserves_leading_underscore() {
        assert_eq!(hook_base("controller", "_ready"), "controller-_ready");
    }

    #[test]
    fn hook_flags_all_sets_every_kind() {
        let f = HookFlags::all();
        assert!(f.pre && f.post && f.callback && f.replace);
    }
}
