//! Per-script ZIP entry model.
//!
//! A [`RewrittenScript`] carries enough information for the pack emitter to
//! produce its 3-entry recipe (`.gd` + `.gd.remap` + empty `.gdc`).

use crabby_error::{CrabbyError, Result};

/// One rewritten script to be packed.
#[derive(Debug, Clone)]
pub struct RewrittenScript {
    /// Forward-slash-separated path within the pack, relative to the
    /// archive root. Typically `"Scripts/<Name>.gd"`. Must not contain
    /// backslashes (which break Godot's pack mount on Windows).
    pub zip_path: String,
    /// Rewritten `GDScript` source, UTF-8.
    pub rewritten_source: String,
    /// Whether to ship the empty `.gdc` companion alongside `.gd` and
    /// `.gd.remap`.
    ///
    /// **For Node-typed scripts (Control, Node3D, etc.): `true`.** The
    /// empty `.gdc` shadows vanilla PCK's compiled bytecode at
    /// path-resolution time, forcing Godot to fall through to our
    /// `.remap` → `.gd` source. Without it the vanilla bytecode wins
    /// and our wrappers never fire.
    ///
    /// **For Resource-typed scripts (additive: SlotData, ItemData,
    /// etc.): `false`.** Shipping the empty `.gdc` in this case
    /// breaks Godot's resource-script class binding - `.tres` files
    /// referencing the path resolve to a generic `Resource` object
    /// without the script class bound, so methods like `Update` /
    /// `_rtv_hooked_*` aren't reachable. Symptom: SlotData has null
    /// `itemData` after `Update` "ran", `set_name("")` flood as
    /// downstream code reads stale fields. Confirmed against the live
    /// game on RTV 4.6.2.
    pub emit_empty_gdc: bool,
}

impl RewrittenScript {
    /// Validate invariants on the `zip_path`:
    ///
    /// - forward slashes only (no `\\`)
    /// - ends with `.gd` (so the `.remap` and `.gdc` companion paths can be derived)
    /// - non-empty
    //
    // Godot resource paths are deliberately case-sensitive; the lint's
    // preferred case-insensitive comparison would be wrong for our domain.
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    pub fn validate(&self) -> Result<()> {
        if self.zip_path.is_empty() {
            return Err(CrabbyError::Pack {
                context: "empty zip_path".into(),
                source: "a RewrittenScript must declare a target path".into(),
            });
        }
        if self.zip_path.contains('\\') {
            return Err(CrabbyError::Pack {
                context: format!("{:?} contains backslash separator", self.zip_path),
                source: "Windows-style paths break Godot's pack mount".into(),
            });
        }
        if !self.zip_path.ends_with(".gd") {
            return Err(CrabbyError::Pack {
                context: format!("{:?} does not end with .gd", self.zip_path),
                source: "crabby packs rewritten .gd scripts only".into(),
            });
        }
        Ok(())
    }

    /// The companion `.gd.remap` entry path.
    #[must_use]
    pub fn remap_entry(&self) -> String {
        format!("{}.remap", self.zip_path)
    }

    /// The companion empty `.gdc` entry path.
    #[must_use]
    pub fn gdc_entry(&self) -> String {
        // `.gd` → `.gdc` by replacing the final `.gd` suffix.
        let stem = self.zip_path.strip_suffix(".gd").unwrap_or(&self.zip_path);
        format!("{stem}.gdc")
    }

    /// Godot resource path the remap entry must point at, e.g.
    /// `res://Scripts/Controller.gd`.
    #[must_use]
    pub fn res_path(&self) -> String {
        format!("res://{}", self.zip_path)
    }

    /// Contents of the `.gd.remap` file - a self-referencing redirect that
    /// supersedes the vanilla PCK's `.gd → .gdc` remap.
    #[must_use]
    pub fn remap_contents(&self) -> String {
        format!("[remap]\npath=\"{}\"\n", self.res_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> RewrittenScript {
        RewrittenScript {
            zip_path: "Scripts/Controller.gd".into(),
            rewritten_source: "extends Node\n".into(),
            emit_empty_gdc: true,
        }
    }

    #[test]
    fn companion_paths_derived_from_gd_path() {
        let r = fixture();
        assert_eq!(r.remap_entry(), "Scripts/Controller.gd.remap");
        assert_eq!(r.gdc_entry(), "Scripts/Controller.gdc");
        assert_eq!(r.res_path(), "res://Scripts/Controller.gd");
    }

    #[test]
    fn remap_points_at_self() {
        let r = fixture();
        assert_eq!(
            r.remap_contents(),
            "[remap]\npath=\"res://Scripts/Controller.gd\"\n",
        );
    }

    #[test]
    fn validate_rejects_backslash() {
        let mut r = fixture();
        r.zip_path = r"Scripts\Controller.gd".into();
        let err = r.validate().expect_err("should reject");
        assert!(format!("{err}").contains("backslash"), "got: {err}");
    }

    #[test]
    fn validate_rejects_non_gd_extension() {
        let mut r = fixture();
        r.zip_path = "Scripts/Controller.tscn".into();
        assert!(r.validate().is_err());
    }

    #[test]
    fn validate_rejects_empty_path() {
        let mut r = fixture();
        r.zip_path = String::new();
        assert!(r.validate().is_err());
    }

    #[test]
    fn validate_accepts_simple_paths() {
        let r = fixture();
        r.validate().expect("should accept");
    }
}
