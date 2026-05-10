//! [`BakeKey`], the single value that identifies a bake.
//!
//! Two bakes with the same key produce byte-equivalent packs (modulo
//! file-system metadata). When `crabby-install` finds a matching key in
//! its manifest it skips the rebake and reuses the cached pack.
//!
//! # What's in the key
//!
//! - **crabby version**, since wrapper templates and injection logic
//!   change with crabby releases
//! - **PCK mtime + size**, since the game's vanilla source is the
//!   rewriter's input; if the PCK changes, rewrites are stale
//! - **mods digest**, a stable hash of the enabled mod set and the
//!   per-method hook flags those mods register. Toggling a mod, adding
//!   a hook to an already-enabled mod, or replacing a mod's archive all
//!   shift this digest, producing a key mismatch and a bake-out-of-date
//!   state where relaunch refuses or auto-bakes (depending on caller
//!   policy). Empty mods produce an empty digest, leaving the key shape
//!   stable for the no-mod baseline.

use std::path::Path;

use crabby_error::{CrabbyError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Opaque key identifying a bake. Compare with `==` for cache lookups.
///
/// Serialized as a compact string:
/// `"<crabby_version>:<pck_mtime>:<pck_size>:<mods_digest>"`.
/// Callers should treat it as an opaque token, since the string shape
/// may change without notice as bake inputs evolve.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BakeKey(String);

impl BakeKey {
    /// Compute the key from its inputs.
    ///
    /// `mods_digest` is the empty string when the caller has nothing
    /// to contribute (e.g. test fixtures, CLI invocations without
    /// analyzer data), in which case the key keeps the legacy
    /// three-component shape.
    #[must_use]
    pub fn new(
        crabby_version: &str,
        pck_mtime_secs: u64,
        pck_size_bytes: u64,
        mods_digest: &str,
    ) -> Self {
        Self(format!(
            "{crabby_version}:{pck_mtime_secs}:{pck_size_bytes}:{mods_digest}"
        ))
    }

    /// Compute the key from on-disk PCK metadata, with no mods
    /// contribution (`mods_digest = ""`). Use [`Self::from_pck_with_mods`]
    /// when you have an enabled-mod set to fold in.
    ///
    /// # Errors
    ///
    /// [`CrabbyError::Io`] when the PCK cannot be `stat`'d.
    pub fn from_pck(crabby_version: &str, pck_path: &Path) -> Result<Self> {
        Self::from_pck_with_mods(crabby_version, pck_path, "")
    }

    /// Compute the key from on-disk PCK metadata plus a precomputed
    /// mods digest. The digest should be stable across runs given the
    /// same inputs (see [`mods_digest_from_kinds`]).
    ///
    /// # Errors
    ///
    /// [`CrabbyError::Io`] when the PCK cannot be `stat`'d.
    pub fn from_pck_with_mods(
        crabby_version: &str,
        pck_path: &Path,
        mods_digest: &str,
    ) -> Result<Self> {
        let meta = pck_path
            .metadata()
            .map_err(|source| CrabbyError::io_at(pck_path.to_path_buf(), source))?;
        let mtime_secs = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        let size = meta.len();
        Ok(Self::new(crabby_version, mtime_secs, size, mods_digest))
    }

    /// Opaque string view, suitable for manifest storage.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Compute a stable digest folding together (a) the per-method hook
/// flags every enabled mod registers, and (b) the set of enabled mod
/// IDs themselves. Caller is `crabby-install`, which collects both
/// from the analyzer's view of the active profile.
///
/// Output is a hex-encoded SHA-256 of the canonicalized input, so:
/// - Same inputs produce the same digest across runs / platforms.
/// - Adding/removing a hook produces a different digest.
/// - Enabling/disabling a hook-bearing mod produces a different digest
///   (via the hook-flags map shrinking).
/// - Enabling/disabling a no-hook mod (pure registry / UI / autoload)
///   produces a different digest (via the enabled-IDs set), even though
///   the wrappers in the bake don't change. This catches the case where
///   two profiles enabled different mods that happened to declare the
///   same hook bases.
/// - Empty hooks AND empty IDs produce the empty string (preserves the
///   legacy three-component key shape for callers without analyzer
///   data, e.g. test fixtures).
#[must_use]
pub fn mods_digest_from_kinds<I, S, J, T>(entries: I, enabled_ids: J) -> String
where
    I: IntoIterator<Item = (S, [bool; 4])>,
    S: AsRef<str>,
    J: IntoIterator<Item = T>,
    T: AsRef<str>,
{
    let mut canonical: Vec<(String, [bool; 4])> = entries
        .into_iter()
        .map(|(k, flags)| (k.as_ref().to_string(), flags))
        .collect();
    let mut ids: Vec<String> = enabled_ids
        .into_iter()
        .map(|s| s.as_ref().to_string())
        .collect();
    if canonical.is_empty() && ids.is_empty() {
        return String::new();
    }
    canonical.sort_by(|a, b| a.0.cmp(&b.0));
    ids.sort();
    ids.dedup();
    let mut hasher = Sha256::new();
    // Section 1: hooks. Layout: <base>:<pre><post><callback><replace>\n
    hasher.update(b"hooks\x00");
    for (base, flags) in &canonical {
        hasher.update(base.as_bytes());
        hasher.update(b":");
        // `flags` order is (pre, post, callback, replace), and callers
        // must preserve this convention so the digest is portable.
        hasher.update([
            flags[0] as u8,
            flags[1] as u8,
            flags[2] as u8,
            flags[3] as u8,
        ]);
        hasher.update(b"\n");
    }
    // Section 2: enabled mod IDs. Folded in even when the hooks map is
    // empty so a pure-registry mod toggling enabled invalidates the key.
    // The `mods\x00` separator prevents collisions between a mod ID and
    // a hook base that happen to share the same bytes.
    hasher.update(b"mods\x00");
    for id in &ids {
        hasher.update(id.as_bytes());
        hasher.update(b"\n");
    }
    let bytes = hasher.finalize();
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

impl std::fmt::Display for BakeKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_inputs_yield_equal_keys() {
        let a = BakeKey::new("0.1.0", 1000, 500, "");
        let b = BakeKey::new("0.1.0", 1000, 500, "");
        assert_eq!(a, b);
    }

    #[test]
    fn version_bump_changes_key() {
        let a = BakeKey::new("0.1.0", 1000, 500, "");
        let b = BakeKey::new("0.2.0", 1000, 500, "");
        assert_ne!(a, b);
    }

    #[test]
    fn pck_mtime_change_changes_key() {
        let a = BakeKey::new("0.1.0", 1000, 500, "");
        let b = BakeKey::new("0.1.0", 1001, 500, "");
        assert_ne!(a, b);
    }

    #[test]
    fn pck_size_change_changes_key() {
        let a = BakeKey::new("0.1.0", 1000, 500, "");
        let b = BakeKey::new("0.1.0", 1000, 501, "");
        assert_ne!(a, b);
    }

    #[test]
    fn mods_digest_change_changes_key() {
        let a = BakeKey::new("0.1.0", 1000, 500, "abc");
        let b = BakeKey::new("0.1.0", 1000, 500, "def");
        assert_ne!(a, b);
    }

    #[test]
    fn roundtrips_through_json() {
        let key = BakeKey::new("0.1.0", 1000, 500, "abc");
        let json = serde_json::to_string(&key).unwrap();
        let back: BakeKey = serde_json::from_str(&json).unwrap();
        assert_eq!(key, back);
    }

    #[test]
    fn mods_digest_empty_input_is_empty_string() {
        let d: String = mods_digest_from_kinds(
            std::iter::empty::<(&str, [bool; 4])>(),
            std::iter::empty::<&str>(),
        );
        assert!(d.is_empty());
    }

    #[test]
    fn mods_digest_is_order_independent() {
        let a = mods_digest_from_kinds(
            [
                ("a-b", [true, false, false, false]),
                ("c-d", [false, true, false, false]),
            ],
            ["mod_a", "mod_b"],
        );
        let b = mods_digest_from_kinds(
            [
                ("c-d", [false, true, false, false]),
                ("a-b", [true, false, false, false]),
            ],
            ["mod_b", "mod_a"],
        );
        assert_eq!(a, b);
    }

    #[test]
    fn mods_digest_distinguishes_flag_changes() {
        let a = mods_digest_from_kinds(
            [("x-y", [true, false, false, false])],
            std::iter::empty::<&str>(),
        );
        let b = mods_digest_from_kinds(
            [("x-y", [true, true, false, false])],
            std::iter::empty::<&str>(),
        );
        assert_ne!(a, b);
    }

    #[test]
    fn mods_digest_distinguishes_enabled_id_changes() {
        // Two profiles with identical hook footprints but different
        // enabled-mod sets must produce different digests. This is the
        // direct regression test for the profile-swap-doesn't-rebake
        // bug, since without the IDs section both calls return the
        // same hash and the install short-circuits to AlreadyCurrent.
        let hooks = [("weapon-fire", [true, false, false, false])];
        let a = mods_digest_from_kinds(hooks, ["mod_a"]);
        let b = mods_digest_from_kinds(hooks, ["mod_b"]);
        assert_ne!(a, b);
    }

    #[test]
    fn mods_digest_distinguishes_set_membership() {
        // Adding a no-hook mod (e.g. a pure-registry one) must shift
        // the digest even when the hooks map is unchanged.
        let hooks = [("weapon-fire", [true, false, false, false])];
        let a = mods_digest_from_kinds(hooks, ["mod_a"]);
        let b = mods_digest_from_kinds(hooks, ["mod_a", "no_hook_mod"]);
        assert_ne!(a, b);
    }

    #[test]
    fn mods_digest_dedups_ids() {
        // Defensive: an analyzer that double-reports an id (shouldn't
        // happen but might during a refactor) must not produce a
        // different digest from the deduped form.
        let hooks: [(&str, [bool; 4]); 0] = [];
        let a = mods_digest_from_kinds(hooks, ["mod_a", "mod_b"]);
        let b = mods_digest_from_kinds(hooks, ["mod_a", "mod_b", "mod_a"]);
        assert_eq!(a, b);
    }

    #[test]
    fn mods_digest_only_ids_still_produces_hash() {
        // No hooks but at least one enabled mod → must still produce
        // a non-empty digest. This is the no-hook-mod case.
        let hooks: [(&str, [bool; 4]); 0] = [];
        let d = mods_digest_from_kinds(hooks, ["mod_a"]);
        assert!(!d.is_empty());
    }
}
