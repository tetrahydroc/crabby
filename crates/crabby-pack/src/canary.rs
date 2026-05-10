//! VFS mount-precedence canary.
//!
//! Embeds a small known-content file in every hook pack. After Godot mounts
//! the pack, the runtime shim reads the canary back via `FileAccess`. A
//! mismatch means `ProjectSettings.load_resource_pack` returned `true` but
//! the overlay isn't actually serving files - a critical failure mode that
//! silently breaks every rewrite.
//!
//! Vostok uses `MODLOADER-VFS-CANARY-<version>`; crabby uses
//! [`CANARY_PREFIX`] followed by its own version so the two loaders'
//! canaries never collide when both are installed side-by-side.

/// File name of the canary entry inside the pack.
pub const CANARY_ENTRY_NAME: &str = "__crabby_canary__.txt";

/// Prefix every canary payload starts with. The runtime shim checks for this
/// prefix and reports the suffix as the version it saw, surfacing stale
/// mounts.
pub const CANARY_PREFIX: &str = "CRABBY-VFS-CANARY-";

/// Build the canary payload for a pack emitted by crabby `version`.
#[must_use]
pub fn canary_content(version: &str) -> String {
    format!("{CANARY_PREFIX}{version}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_starts_with_prefix() {
        assert_eq!(canary_content("0.1.0"), "CRABBY-VFS-CANARY-0.1.0");
    }
}
