//! `AISpawner.gd` injection for the `ai_types` registry.
//!
//! Vanilla `AISpawner.gd` hardcodes a zone -> agent-scene mapping in
//! `_ready()`:
//!
//! ```gdscript
//! if zone == Zone.Area05:
//!     agent = bandit
//! elif zone == Zone.BorderZone:
//!     agent = guard
//! elif zone == Zone.Vostok:
//!     agent = military
//! ```
//!
//! For the `ai_types` registry to work, each `agent = <var>`
//! assignment is wrapped to consult a mod-override dict (broadcast via
//! `Engine.set_meta("_rtv_ai_overrides", ...)`) and falls back to the
//! vanilla scene if no override is registered.
//!
//! ```gdscript
//! agent = _rtv_resolve_ai_type(zone, bandit)
//! ```
//!
//! Plus a `_rtv_resolve_ai_type` helper appended at the end of the file.

use std::fmt::Write as _;
use std::sync::LazyLock;

use regex::Regex;

/// Filename that triggers this transform.
pub const AI_SPAWNER_FILENAME: &str = "AISpawner.gd";

/// Engine.meta key the resolver reads at runtime. Mirrored on the shim
/// side as `_AI_ENGINE_META_KEY`.
pub const AI_ENGINE_META_KEY: &str = "_rtv_ai_overrides";

/// Matches `<indent>agent = <name>` at any indent level. Captures
/// indent (group 1) and the vanilla scene name (group 2).
static AGENT_ASSIGN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^([ \t]*)agent\s*=\s*([A-Za-z_][A-Za-z0-9_]*)\s*$"#).expect("AGENT_ASSIGN regex")
});

/// Apply the AISpawner-side transform to `source`. Pass-through if
/// `filename` doesn't match `AI_SPAWNER_FILENAME` or no `agent = X`
/// lines are found.
#[must_use]
pub fn transform(filename: &str, source: &str) -> String {
    if filename != AI_SPAWNER_FILENAME {
        return source.to_owned();
    }

    let mut out: Vec<String> = Vec::with_capacity(source.lines().count() + 16);
    let mut wrapped = 0usize;
    for line in source.lines() {
        if let Some(caps) = AGENT_ASSIGN.captures(line) {
            let indent = caps.get(1).map_or("", |m| m.as_str());
            let scene = caps.get(2).map_or("", |m| m.as_str());
            out.push(format!(
                "{indent}agent = _rtv_resolve_ai_type(zone, {scene})",
            ));
            wrapped += 1;
        } else {
            out.push(line.to_owned());
        }
    }

    if wrapped == 0 {
        // No assignments matched, return source verbatim. Guards against
        // a future RTV refactor of AISpawner emitting a corrupted file.
        return source.to_owned();
    }

    let mut s = out.join("\n");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s.push_str(&resolver_helper());
    s
}

/// Resolver appended at the end of the file. Consults engine-meta dict
/// keyed by Zone enum name (e.g. "Area05") and returns the override
/// scene if present, else the vanilla fallback.
fn resolver_helper() -> String {
    let mut s = String::new();
    let _ = writeln!(s);
    let _ = writeln!(s, "# --- Crabby ai_types resolver ---");
    let _ = writeln!(s, "func _rtv_resolve_ai_type(zone_value, vanilla_scene):");
    let _ = writeln!(
        s,
        "\tvar overrides: Dictionary = Engine.get_meta(\"{AI_ENGINE_META_KEY}\", {{}})",
    );
    let _ = writeln!(s, "\tif overrides.is_empty():");
    let _ = writeln!(s, "\t\treturn vanilla_scene");
    let _ = writeln!(s, "\tvar zone_name: String = Zone.keys()[int(zone_value)]");
    let _ = writeln!(s, "\tif overrides.has(zone_name):");
    let _ = writeln!(s, "\t\treturn overrides[zone_name]");
    let _ = writeln!(s, "\treturn vanilla_scene");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_for_non_aispawner_files() {
        let src = "extends Node\n\nfunc foo(): agent = bandit\n";
        let out = transform("Other.gd", src);
        assert_eq!(out, src);
    }

    #[test]
    fn wraps_each_agent_assignment() {
        let src = "extends Node3D\n\nfunc _ready():\n\tif zone == Zone.Area05:\n\t\tagent = bandit\n\telif zone == Zone.BorderZone:\n\t\tagent = guard\n\telif zone == Zone.Vostok:\n\t\tagent = military\n";
        let out = transform("AISpawner.gd", src);
        assert!(
            out.contains("agent = _rtv_resolve_ai_type(zone, bandit)"),
            "{out}"
        );
        assert!(
            out.contains("agent = _rtv_resolve_ai_type(zone, guard)"),
            "{out}"
        );
        assert!(
            out.contains("agent = _rtv_resolve_ai_type(zone, military)"),
            "{out}"
        );
    }

    #[test]
    fn appends_resolver_helper() {
        let src = "extends Node3D\n\nfunc _ready():\n\tagent = bandit\n";
        let out = transform("AISpawner.gd", src);
        assert!(
            out.contains("func _rtv_resolve_ai_type(zone_value, vanilla_scene)"),
            "{out}"
        );
        assert!(
            out.contains("Engine.get_meta(\"_rtv_ai_overrides\", {})"),
            "{out}"
        );
        assert!(out.contains("Zone.keys()[int(zone_value)]"), "{out}");
    }

    #[test]
    fn no_assignments_means_no_changes() {
        let src = "extends Node3D\n\nfunc _ready():\n\tpass\n";
        let out = transform("AISpawner.gd", src);
        assert_eq!(out, src);
    }
}
