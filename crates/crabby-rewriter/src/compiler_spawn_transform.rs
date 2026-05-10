//! `Compiler.gd::Spawn()` injection for B_Loader-style shelters/maps.
//!
//! Vanilla `Compiler.Spawn()` resolves world transitions via a giant
//! if-elif chain on `map.mapName`:
//!
//! ```gdscript
//! func Spawn():
//!     var spawnTarget: String
//!     var spawnPoint: Node3D
//!     var map = get_tree().current_scene.get_node("/root/Map")
//!     var transitions = get_tree().get_nodes_in_group("Transition")
//!     var waypoints = get_tree().get_nodes_in_group("AI_WP")
//!     ...
//!     elif map.mapName == "Cabin":
//!         Loader.LoadWorld()
//!         Loader.LoadCharacter()
//!         Loader.LoadShelter("Cabin")
//!         Simulation.simulate = true
//!         spawnTarget = "Door_Cabin_Exit"
//!     ...
//! ```
//!
//! For mod-registered shelters/maps to participate in this routing, a
//! prelude is injected immediately after the leading var declarations
//! (so `spawnTarget`/`transitions`/`waypoints` are in scope) that
//! handles two B_Loader-style cases:
//!
//! 1. **Player just transitioned INTO a registered shelter or map.**
//!    Run the vanilla load sequence (LoadWorld + LoadCharacter +
//!    optional LoadShelter + Simulation.simulate=true), set
//!    `spawnTarget` to the entry's `exit_spawn`, run the transition
//!    pose loop locally, fire the gameData.* resets, and `return`
//!    so vanilla's if-elif chain doesn't double-process.
//!
//! 2. **Player transitioned INTO a vanilla map that has at least one
//!    registered shelter/map hanging off it via `connected_to`.**
//!    Spawn the `connected_content` props into `/root/Map/Content`
//!    additively. Refresh the `transitions` / `waypoints` locals
//!    (vanilla snapshotted them at the top of `Spawn()` BEFORE these
//!    additions). If the player is arriving FROM a registered
//!    shelter, pre-set `spawnTarget` to its `entrance_spawn` so
//!    vanilla's if-elif (which only knows vanilla map names) won't
//!    overwrite it. Fall through to vanilla for the rest.

use std::sync::LazyLock;

use regex::Regex;

/// Filename that triggers this transform.
pub const COMPILER_FILENAME: &str = "Compiler.gd";

/// Matches the `func Spawn():` line.
static SPAWN_FN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^func\s+Spawn\s*\(\s*\)\s*:\s*$"#).expect("SPAWN_FN regex"));

/// Apply the Compiler.Spawn-side transform to `source`. Pass-through
/// if `filename` doesn't match `COMPILER_FILENAME` or `Spawn` isn't
/// found.
#[must_use]
pub fn transform(filename: &str, source: &str, indent: &str) -> String {
    if filename != COMPILER_FILENAME {
        return source.to_owned();
    }

    let lines: Vec<&str> = source.lines().collect();
    let mut prelude_inserted = false;
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 64);

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        out.push(line.to_owned());

        if !prelude_inserted && SPAWN_FN.is_match(line) {
            // Skip past the run of leading body lines that are blank
            // or `var ...` declarations. Insert the prelude AFTER the
            // last var so spawnTarget/transitions/waypoints are in
            // scope. Stop on the first indented line that isn't a var,
            // OR on a top-level line (function body ended).
            i += 1;
            let mut last_var_idx: Option<usize> = None;
            while i < lines.len() {
                let next = lines[i];
                let stripped = next.trim_start();
                if stripped.is_empty() {
                    out.push(next.to_owned());
                    i += 1;
                    continue;
                }
                // Top-level line means body ended.
                if !next.starts_with(' ') && !next.starts_with('\t') {
                    break;
                }
                if stripped.starts_with("var ") {
                    out.push(next.to_owned());
                    last_var_idx = Some(out.len() - 1);
                    i += 1;
                    continue;
                }
                // First non-var body line, stop, the prelude lands here.
                break;
            }
            // If vars were found, anchor the prelude after the last one.
            // Otherwise (degenerate Spawn), fall back to right after
            // the signature.
            let _ = last_var_idx;
            for prelude in spawn_prelude_lines(indent) {
                out.push(prelude);
            }
            prelude_inserted = true;
            continue;
        }
        i += 1;
    }

    if !prelude_inserted {
        // No Spawn() found, pass through to avoid corrupting the file.
        return source.to_owned();
    }

    let mut s = out.join("\n");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// The Compiler.Spawn prelude. Reads from `Loader._rtv_mod_shelters`
/// (injected by `loader_transform`) and dispatches into the two
/// B_Loader cases. Mutates the locals declared above (`spawnTarget`,
/// `transitions`, `waypoints`).
fn spawn_prelude_lines(indent: &str) -> Vec<String> {
    let i1 = indent;
    let i2 = format!("{indent}{indent}");
    let i3 = format!("{indent}{indent}{indent}");
    let i4 = format!("{indent}{indent}{indent}{indent}");
    let i5 = format!("{indent}{indent}{indent}{indent}{indent}");
    let i6 = format!("{indent}{indent}{indent}{indent}{indent}{indent}");

    let mut p: Vec<String> = Vec::with_capacity(64);
    p.push(format!("{i1}# --- Crabby shelters/maps registry prelude ---"));
    p.push(format!("{i1}var _rtv_map_node: Node = get_tree().current_scene.get_node_or_null(\"/root/Map\")"));
    p.push(format!("{i1}if _rtv_map_node != null and \"_rtv_mod_shelters\" in Loader:"));
    p.push(format!("{i2}var _rtv_mn: String = String(_rtv_map_node.mapName)"));
    p.push(format!("{i2}var _rtv_entry: Dictionary = Loader._rtv_mod_shelters.get(_rtv_mn, {{}})"));
    // Case 1: arriving in a registered shelter / map.
    p.push(format!("{i2}if not _rtv_entry.is_empty():"));
    p.push(format!("{i3}Loader.LoadWorld()"));
    p.push(format!("{i3}Loader.LoadCharacter()"));
    p.push(format!("{i3}if bool(_rtv_entry.get(\"shelter\", false)):"));
    p.push(format!("{i4}Loader.LoadShelter(_rtv_mn)"));
    p.push(format!("{i3}Simulation.simulate = true"));
    p.push(format!("{i3}spawnTarget = String(_rtv_entry.get(\"exit_spawn\", \"\"))"));
    // Run the transition-pose loop locally to enable early-return.
    // Reuses vanilla's `transitions` local declared above.
    p.push(format!("{i3}if spawnTarget != \"\":"));
    p.push(format!("{i4}for _rtv_t in transitions:"));
    p.push(format!("{i5}if _rtv_t.owner.name == spawnTarget:"));
    p.push(format!("{i6}var _rtv_sp = _rtv_t.owner.spawn"));
    p.push(format!("{i6}if _rtv_sp:"));
    p.push(format!("{i6}{i1}controller.global_transform.basis = _rtv_sp.global_transform.basis"));
    p.push(format!("{i6}{i1}controller.global_transform.basis = controller.global_transform.basis.rotated(Vector3.UP, deg_to_rad(180))"));
    p.push(format!("{i6}{i1}controller.global_position = _rtv_sp.global_position"));
    p.push(format!("{i3}gameData.isTransitioning = false"));
    p.push(format!("{i3}gameData.isSleeping = false"));
    p.push(format!("{i3}gameData.isOccupied = false"));
    p.push(format!("{i3}gameData.freeze = false"));
    p.push(format!("{i3}return"));
    // Case 2: this map is connected_to for one or more registered shelters/maps.
    p.push(format!("{i2}for _rtv_key in Loader._rtv_mod_shelters:"));
    p.push(format!("{i3}var _rtv_e: Dictionary = Loader._rtv_mod_shelters[_rtv_key]"));
    p.push(format!("{i3}if String(_rtv_e.get(\"connected_to\", \"\")) != _rtv_mn:"));
    p.push(format!("{i4}continue"));
    // Spawn connected_content additively into /root/Map/Content.
    p.push(format!("{i3}var _rtv_content: Node = get_tree().current_scene.get_node_or_null(\"/root/Map/Content\")"));
    p.push(format!("{i3}if _rtv_content != null:"));
    p.push(format!("{i4}var _rtv_items: Array = _rtv_e.get(\"connected_content\", [])"));
    p.push(format!("{i4}for _rtv_item in _rtv_items:"));
    p.push(format!("{i5}if not (_rtv_item is Dictionary):"));
    p.push(format!("{i6}continue"));
    p.push(format!("{i5}var _rtv_p: String = String(_rtv_item.get(\"path\", \"\"))"));
    p.push(format!("{i5}if _rtv_p == \"\":"));
    p.push(format!("{i6}continue"));
    p.push(format!("{i5}var _rtv_packed: Variant = load(_rtv_p)"));
    p.push(format!("{i5}if _rtv_packed == null:"));
    p.push(format!("{i6}push_warning(\"[Registry] connected_content: failed to load \" + _rtv_p)"));
    p.push(format!("{i6}continue"));
    p.push(format!("{i5}var _rtv_inst: Node = _rtv_packed.instantiate()"));
    p.push(format!("{i5}if \"position\" in _rtv_item:"));
    p.push(format!("{i6}_rtv_inst.position = _rtv_item[\"position\"]"));
    p.push(format!("{i5}if \"rotation\" in _rtv_item:"));
    p.push(format!("{i6}_rtv_inst.rotation_degrees = _rtv_item[\"rotation\"]"));
    p.push(format!("{i5}_rtv_content.add_child(_rtv_inst)"));
    // Refresh transitions / waypoints to include freshly-spawned nodes.
    p.push(format!("{i3}transitions = get_tree().get_nodes_in_group(\"Transition\")"));
    p.push(format!("{i3}waypoints = get_tree().get_nodes_in_group(\"AI_WP\")"));
    // If player is arriving from this mod shelter, pre-set entrance_spawn.
    p.push(format!("{i3}if String(gameData.previousMap) == _rtv_key:"));
    p.push(format!("{i4}spawnTarget = String(_rtv_e.get(\"entrance_spawn\", \"\"))"));
    p.push(format!("{i1}# Fall through to vanilla if-elif (handles vanilla maps)."));
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_for_non_compiler_files() {
        let src = "extends Node\n\nfunc Spawn(): pass\n";
        let out = transform("Other.gd", src, "\t");
        assert_eq!(out, src);
    }

    #[test]
    fn injects_prelude_after_var_decls() {
        let src = "extends Node\n\nfunc Spawn():\n\tvar spawnTarget: String\n\tvar spawnPoint: Node3D\n\tvar map = get_tree().current_scene.get_node(\"/root/Map\")\n\tvar transitions = get_tree().get_nodes_in_group(\"Transition\")\n\tvar waypoints = get_tree().get_nodes_in_group(\"AI_WP\")\n\n\tif waypoints.size() != 0:\n\t\tcontroller.global_position = waypoints.pick_random().global_position\n";
        let out = transform("Compiler.gd", src, "\t");
        let prelude_pos = out
            .find("Crabby shelters/maps registry prelude")
            .expect("prelude present");
        let last_var_pos = out.find("get_nodes_in_group(\"AI_WP\")").expect("var present");
        let body_pos = out.find("if waypoints.size()").expect("body present");
        assert!(last_var_pos < prelude_pos, "prelude must follow last var\n{out}");
        assert!(prelude_pos < body_pos, "prelude must precede body\n{out}");
    }

    #[test]
    fn prelude_uses_loader_mod_shelters() {
        let src = "extends Node\n\nfunc Spawn():\n\tvar spawnTarget: String\n\tpass\n";
        let out = transform("Compiler.gd", src, "\t");
        assert!(
            out.contains("\"_rtv_mod_shelters\" in Loader"),
            "{out}",
        );
        assert!(out.contains("Loader._rtv_mod_shelters.get(_rtv_mn"), "{out}");
    }

    #[test]
    fn prelude_handles_arriving_in_mod_shelter() {
        let src = "extends Node\n\nfunc Spawn():\n\tvar spawnTarget: String\n\tpass\n";
        let out = transform("Compiler.gd", src, "\t");
        assert!(out.contains("Loader.LoadShelter(_rtv_mn)"), "{out}");
        assert!(out.contains("spawnTarget = String(_rtv_entry.get(\"exit_spawn\""), "{out}");
        assert!(out.contains("gameData.isTransitioning = false"), "{out}");
    }

    #[test]
    fn prelude_handles_connected_content_spawn() {
        let src = "extends Node\n\nfunc Spawn():\n\tvar spawnTarget: String\n\tpass\n";
        let out = transform("Compiler.gd", src, "\t");
        assert!(out.contains("connected_content"), "{out}");
        assert!(out.contains("/root/Map/Content"), "{out}");
        assert!(out.contains("transitions = get_tree().get_nodes_in_group(\"Transition\")"), "{out}");
        assert!(out.contains("entrance_spawn"), "{out}");
    }

    #[test]
    fn no_spawn_means_no_changes() {
        let src = "extends Node\n\nfunc _ready(): pass\n";
        let out = transform("Compiler.gd", src, "\t");
        assert_eq!(out, src);
    }
}
