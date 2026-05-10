//! `Loader.gd` injection for the `scene_paths` / `shelters` /
//! `random_scenes` registries.
//!
//! Vanilla `Loader.gd` resolves scene names via a giant if-elif chain
//! inside `LoadScene(scene: String)`:
//!
//! ```gdscript
//! if scene == "Cabin":
//!     scenePath = Cabin
//!     gameData.menu = false
//!     gameData.shelter = true
//!     ...
//! ```
//!
//! For mods to register their own scene names + flag combinations, a
//! prelude is injected at the top of `LoadScene` that consults two
//! injected dicts (override > mod) and short-circuits the if-elif if
//! either matches. The vanilla chain still runs for vanilla names.
//!
//! Vanilla `const shelters = [...]` is also rewritten to a snapshotted
//! `var shelters = [...]` so the shelters registry can append at
//! runtime. `randomScenes` is already a `var` in vanilla; no rewrite
//! needed there, the random_scenes registry just appends.
//!
//! # Injection shape
//!
//! Inserted near the top of the file (after the existing imports/vars,
//! before any function):
//!
//! ```gdscript
//! var _rtv_mod_scene_paths: Dictionary = {}
//! var _rtv_override_scene_paths: Dictionary = {}
//! var _rtv_vanilla_shelters: Array = ["Cabin", "Attic", "Classroom", "Tent", "Bunker"]
//! ```
//!
//! Inserted at the top of `LoadScene(scene: String):`:
//!
//! ```gdscript
//! var _rtv_resolved = _rtv_resolve_scene_path(scene)
//! if _rtv_resolved.size() > 0:
//!     scenePath = _rtv_resolved["path"]
//!     gameData.menu = _rtv_resolved.get("menu", false)
//!     gameData.shelter = _rtv_resolved.get("shelter", false)
//!     gameData.permadeath = _rtv_resolved.get("permadeath", false)
//!     gameData.tutorial = _rtv_resolved.get("tutorial", false)
//!     <existing FadeInLoading + freeze + label setup>
//!     return
//! ```
//!
//! Plus `_rtv_resolve_scene_path` appended at the end of the file.

use std::fmt::Write as _;
use std::sync::LazyLock;

use regex::Regex;

/// Filename that triggers this transform.
pub const LOADER_FILENAME: &str = "Loader.gd";

/// Name of the dict holding mod-registered scene-path entries.
pub const MOD_SCENE_PATHS_VAR: &str = "_rtv_mod_scene_paths";
/// Name of the dict holding mod overrides of vanilla scene-name entries.
pub const OVERRIDE_SCENE_PATHS_VAR: &str = "_rtv_override_scene_paths";
/// Name of the dict holding rich shelter/map entries (B_Loader-style:
/// transition_text, exit_spawn, entrance_spawn, connected_to,
/// connected_content, shelter:bool). Consumed by Compiler.Spawn's
/// prelude at world-transition time.
pub const MOD_SHELTERS_VAR: &str = "_rtv_mod_shelters";
/// Name of the array holding the vanilla shelter names (snapshot for revert).
pub const VANILLA_SHELTERS_VAR: &str = "_rtv_vanilla_shelters";

/// Matches `const shelters = [...]` at module scope, capturing the
/// array literal (group 1).
static SHELTERS_CONST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^const\s+shelters\s*=\s*(\[[^\]]*\])\s*$"#).expect("SHELTERS_CONST regex")
});

/// Matches the LoadScene function signature line.
static LOAD_SCENE_FN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^func\s+LoadScene\s*\(\s*scene\s*:\s*String\s*\)\s*:\s*$"#)
        .expect("LOAD_SCENE_FN regex")
});

/// Matches the bare `_ready()` function signature line. Indent-tolerant
/// so a tabs-vs-spaces flip in vanilla doesn't break the anchor.
static READY_FN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^func\s+_ready\s*\(\s*\)\s*:\s*$"#).expect("READY_FN regex"));

/// Apply the Loader-side transforms to `source`. Returns the rewritten
/// source verbatim if the file isn't `Loader.gd` or if expected anchors
/// aren't found (pass-through preferred to corrupting vanilla).
#[must_use]
pub fn transform(filename: &str, source: &str, indent: &str) -> String {
    if filename != LOADER_FILENAME {
        return source.to_owned();
    }

    // Pre-pass: rewrite the per-save path literals + DirAccess.open
    // calls so save IO resolves against `user://saves/<active_slot>/`
    // instead of `user://`. Validator + Preferences stay at the root
    // (they're install-global, not per-slot).
    let source = rewrite_save_paths(source);
    let lines: Vec<&str> = source.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 32);

    let mut shelters_list: Option<String> = None;
    let mut shelters_replaced = false;
    let mut load_scene_prelude_inserted = false;
    let mut ready_prelude_inserted = false;

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];

        // Rewrite `const shelters = [...]` -> snapshot var + mutable var.
        if !shelters_replaced
            && let Some(caps) = SHELTERS_CONST.captures(line)
        {
            let array_lit = caps.get(1).map_or("[]", |m| m.as_str()).to_string();
            shelters_list = Some(array_lit.clone());
            out.push(format!("var {VANILLA_SHELTERS_VAR}: Array = {array_lit}"));
            out.push(format!(
                "var shelters: Array = {VANILLA_SHELTERS_VAR}.duplicate()",
            ));
            shelters_replaced = true;
            i += 1;
            continue;
        }

        // Inject prelude at top of LoadScene body.
        if !load_scene_prelude_inserted && LOAD_SCENE_FN.is_match(line) {
            out.push(line.to_owned());
            load_scene_prelude_inserted = true;
            i += 1;
            // Emit prelude lines before whatever follows.
            for prelude in load_scene_prelude_lines(indent) {
                out.push(prelude);
            }
            continue;
        }

        // Inject save-slot init at top of `_ready()`. Vanilla `_ready`
        // is a one-liner that sets master amplify volume; the call
        // lands before it so `_rtv_save_slot` is populated by the time
        // anything else needs a save path.
        if !ready_prelude_inserted && READY_FN.is_match(line) {
            out.push(line.to_owned());
            ready_prelude_inserted = true;
            i += 1;
            out.push(format!("{indent}_rtv_init_save_slot()"));
            continue;
        }

        out.push(line.to_owned());
        i += 1;
    }

    // Inject the dict declarations once, after the LAST top-level `var`
    // or `const` line at module scope (i.e., before the first `func`).
    // Scanning from the start finds the first `func` line; insertion
    // lands just before it.
    let dicts_block = inject_dicts_block();
    let insert_at = first_func_line(&out);
    out.splice(insert_at..insert_at, dicts_block);

    // Append the resolver helper + B_Loader compat shim + save-slot
    // helpers at the very end.
    let mut s = out.join("\n");
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s.push_str(&resolver_helper());
    s.push_str(&bloader_compat_shim(indent));
    s.push_str(&save_slot_helpers(indent));

    let _ = shelters_list; // captured for diagnostics; unused beyond carry.
    s
}

/// Build the per-line block spliced in just before the first `func`.
fn inject_dicts_block() -> Vec<String> {
    vec![
        String::from("# --- Crabby loader-registry injection ---"),
        format!("var {MOD_SCENE_PATHS_VAR}: Dictionary = {{}}"),
        format!("var {OVERRIDE_SCENE_PATHS_VAR}: Dictionary = {{}}"),
        format!("var {MOD_SHELTERS_VAR}: Dictionary = {{}}"),
        String::new(),
    ]
}

/// Indentation-aware prelude for the top of `LoadScene`. Sets
/// scenePath + gameData flags from the resolved entry, optionally
/// reassigns the `scene` arg from `transition_text` so the loading
/// label uses the modder's preferred name, then **falls through**.
///
/// Falling through is critical: vanilla's tail (after the if-elif
/// chain) calls `FadeInLoading()`, sets the label, AND calls
/// `change_scene_to_file(scenePath)`. An early return would skip
/// the actual scene change and the game would hang on the loading
/// screen forever.
///
/// Vanilla's if-elif checks `scene == "Menu"` etc. against String
/// literal names; mod-registered names won't match any of them, so
/// the if-elif body is harmlessly skipped and execution lands at the
/// common tail with the resolved scenePath in place.
fn load_scene_prelude_lines(indent: &str) -> Vec<String> {
    let i1 = indent;
    vec![
        format!("{i1}var _rtv_resolved: Dictionary = _rtv_resolve_scene_path(scene)"),
        format!("{i1}if not _rtv_resolved.is_empty():"),
        format!("{i1}{i1}scenePath = _rtv_resolved.get(\"path\", \"\")"),
        format!("{i1}{i1}gameData.menu = _rtv_resolved.get(\"menu\", false)"),
        format!("{i1}{i1}gameData.shelter = _rtv_resolved.get(\"shelter\", false)"),
        format!("{i1}{i1}gameData.permadeath = _rtv_resolved.get(\"permadeath\", false)"),
        format!("{i1}{i1}gameData.tutorial = _rtv_resolved.get(\"tutorial\", false)"),
        // B_Loader compat: transition_text overrides the loading-screen
        // label. Vanilla never reads `scene` again after the label
        // line, so reassigning here is safe.
        format!("{i1}{i1}var _rtv_label: String = String(_rtv_resolved.get(\"transition_text\", \"\"))"),
        format!("{i1}{i1}if _rtv_label != \"\":"),
        format!("{i1}{i1}{i1}scene = _rtv_label"),
        format!("{i1}{i1}# Fall through to vanilla tail (FadeInLoading + label + change_scene_to_file)."),
    ]
}

/// Resolver helper appended at the end of the file. Walks override
/// then mod dicts, returns the matched dict (with the `path` key) or
/// an empty dict on miss.
fn resolver_helper() -> String {
    let mut s = String::new();
    let _ = writeln!(s, "");
    let _ = writeln!(s, "# --- Crabby loader-registry helper ---");
    let _ = writeln!(s, "func _rtv_resolve_scene_path(scene: String) -> Dictionary:");
    let _ = writeln!(
        s,
        "\tif {OVERRIDE_SCENE_PATHS_VAR}.has(scene):",
    );
    let _ = writeln!(s, "\t\treturn {OVERRIDE_SCENE_PATHS_VAR}[scene]");
    let _ = writeln!(s, "\tif {MOD_SCENE_PATHS_VAR}.has(scene):");
    let _ = writeln!(s, "\t\treturn {MOD_SCENE_PATHS_VAR}[scene]");
    let _ = writeln!(s, "\treturn {{}}");
    s
}

/// B_Loader compat shim methods (`add_shelter` / `add_map`) appended
/// to the Loader class. Mods written against BitByteBytes/B_Loader
/// call these directly on the Loader autoload instead of going through
/// `lib.register('shelters'/'maps', ...)`. The shim translates the
/// legacy dict shape (uses `map_name` instead of an explicit id, and
/// `scene_path` instead of `path`) to the internal entry format and
/// writes to the same dicts that the registry handlers write to.
fn bloader_compat_shim(indent: &str) -> String {
    let mut s = String::new();
    let i1 = indent;
    let i2 = format!("{indent}{indent}");
    let i3 = format!("{indent}{indent}{indent}");

    let _ = writeln!(s);
    let _ = writeln!(s, "# --- Crabby B_Loader compat shim ---");
    let _ = writeln!(s, "func add_shelter(d: Dictionary) -> bool:");
    let _ = writeln!(s, "{i1}return _rtv_bloader_compat_register(d, true)");
    let _ = writeln!(s);
    let _ = writeln!(s, "func add_map(d: Dictionary) -> bool:");
    let _ = writeln!(s, "{i1}return _rtv_bloader_compat_register(d, false)");
    let _ = writeln!(s);
    let _ = writeln!(s, "func _rtv_bloader_compat_register(d: Dictionary, default_shelter: bool) -> bool:");
    let _ = writeln!(s, "{i1}if not (d is Dictionary):");
    let _ = writeln!(s, "{i2}push_warning(\"[B_Loader compat] add_shelter/add_map expects a Dictionary\")");
    let _ = writeln!(s, "{i2}return false");
    let _ = writeln!(s, "{i1}var id: String = String(d.get(\"map_name\", \"\"))");
    let _ = writeln!(s, "{i1}if id == \"\":");
    let _ = writeln!(s, "{i2}push_warning(\"[B_Loader compat] dict is missing 'map_name'\")");
    let _ = writeln!(s, "{i2}return false");
    let _ = writeln!(s, "{i1}if {MOD_SHELTERS_VAR}.has(id):");
    let _ = writeln!(s, "{i2}push_warning(\"[B_Loader compat] '\" + id + \"' already registered\")");
    let _ = writeln!(s, "{i2}return false");
    let _ = writeln!(s, "{i1}if id in shelters:");
    let _ = writeln!(s, "{i2}push_warning(\"[B_Loader compat] '\" + id + \"' already in vanilla shelters list\")");
    let _ = writeln!(s, "{i2}return false");
    let _ = writeln!(s, "{i1}var is_shelter: bool = bool(d.get(\"shelter\", default_shelter))");
    // B_Loader uses 'scene_path'; both are accepted.
    let _ = writeln!(s, "{i1}var scene_path: String = String(d.get(\"path\", d.get(\"scene_path\", \"\")))");
    let _ = writeln!(s, "{i1}var entry: Dictionary = {{");
    let _ = writeln!(s, "{i2}\"shelter\": is_shelter,");
    let _ = writeln!(s, "{i2}\"transition_text\": String(d.get(\"transition_text\", id)),");
    let _ = writeln!(s, "{i2}\"exit_spawn\": String(d.get(\"exit_spawn\", \"\")),");
    let _ = writeln!(s, "{i2}\"entrance_spawn\": String(d.get(\"entrance_spawn\", \"\")),");
    let _ = writeln!(s, "{i2}\"connected_to\": String(d.get(\"connected_to\", \"\")),");
    let _ = writeln!(s, "{i2}\"connected_content\": d.get(\"connected_content\", []),");
    let _ = writeln!(s, "{i1}}}");
    let _ = writeln!(s, "{i1}{MOD_SHELTERS_VAR}[id] = entry");
    let _ = writeln!(s, "{i1}shelters.append(id)");
    // Auto-register paired scene_paths entry if scene_path was provided.
    let _ = writeln!(s, "{i1}if scene_path != \"\":");
    let _ = writeln!(s, "{i2}var sp: Dictionary = {{");
    let _ = writeln!(s, "{i3}\"path\": scene_path,");
    let _ = writeln!(s, "{i3}\"shelter\": is_shelter,");
    let _ = writeln!(s, "{i3}\"transition_text\": entry[\"transition_text\"],");
    let _ = writeln!(s, "{i2}}}");
    let _ = writeln!(s, "{i2}if d.has(\"menu\"): sp[\"menu\"] = d[\"menu\"]");
    let _ = writeln!(s, "{i2}if d.has(\"permadeath\"): sp[\"permadeath\"] = d[\"permadeath\"]");
    let _ = writeln!(s, "{i2}if d.has(\"tutorial\"): sp[\"tutorial\"] = d[\"tutorial\"]");
    let _ = writeln!(s, "{i2}{MOD_SCENE_PATHS_VAR}[id] = sp");
    let _ = writeln!(s, "{i1}print(\"[B_Loader compat] registered '\" + id + \"' (shelter=\" + str(is_shelter) + \", connected_to='\" + entry[\"connected_to\"] + \"')\")");
    let _ = writeln!(s, "{i1}return true");
    s
}

/// Rewrite vanilla's per-slot save-path string literals so they resolve
/// against `user://saves/<active_slot>/` instead of `user://`.
///
/// Touched paths (slot-eligible, game state):
/// - `"user://Character.tres"` -> `_rtv_save_path("Character.tres")`
/// - `"user://World.tres"`     -> `_rtv_save_path("World.tres")`
/// - `"user://Traders.tres"`   -> `_rtv_save_path("Traders.tres")`
/// - `"user://Cabin.tres"`     -> `_rtv_save_path("Cabin.tres")`
/// - `"user://Tent.tres"`      -> `_rtv_save_path("Tent.tres")`
/// - `"user://" + targetShelter + ".tres"` -> `_rtv_save_path(targetShelter + ".tres")`
/// - `DirAccess.open("user://")` -> `DirAccess.open(_rtv_save_dir())`
///
/// Untouched (install-global, not per-slot):
/// - `"user://Validator.tres"` (schema/version marker)
/// - `"user://Preferences.tres"` (settings persist across slots)
/// - The `"user://"` literal inside the per-slot DirAccess scans is
///   replaced via the `DirAccess.open("user://")` rule + the trailing
///   `"user://" + file` rewrite. Those `+ file` occurrences become
///   `_rtv_save_dir() + file`.
fn rewrite_save_paths(source: &str) -> String {
    let mut s = source.to_string();

    // Order matters: rewrite the longer/more-specific patterns first so
    // a generic `"user://" + ...` rewrite can't eat a literal that needs
    // individual handling.
    for slot_file in ["Character.tres", "World.tres", "Traders.tres", "Cabin.tres", "Tent.tres"] {
        let from = format!("\"user://{slot_file}\"");
        let to = format!("_rtv_save_path(\"{slot_file}\")");
        s = s.replace(&from, &to);
    }

    // Dynamic shelter writes/reads: `"user://" + targetShelter + ".tres"`.
    s = s.replace(
        "\"user://\" + targetShelter + \".tres\"",
        "_rtv_save_path(targetShelter + \".tres\")",
    );

    // DirAccess scans of the user dir (ValidateShelter / FormatSave /
    // FormatAll). Only Loader.gd usages are targeted; this string
    // never appears outside DirAccess.open in vanilla.
    s = s.replace(
        "DirAccess.open(\"user://\")",
        "DirAccess.open(_rtv_save_dir())",
    );

    // Inside those scans, file paths are built as `"user://" + file`.
    // After the DirAccess rewrite, the loose `"user://" + file` is the
    // last remaining root-prefix concat, point it at the slot dir too.
    s = s.replace("\"user://\" + file", "_rtv_save_dir() + file");

    s
}

/// Build the save-slot helper block appended at the very end of
/// `Loader.gd`. Provides `_rtv_save_dir`, `_rtv_save_path`, and
/// `_rtv_init_save_slot`, plus a one-shot migration that moves loose
/// vanilla saves from `user://` into `user://saves/<slot>/` so existing
/// players don't lose their progress on first launch with the rewrite.
fn save_slot_helpers(indent: &str) -> String {
    let i1 = indent;
    let i2 = format!("{indent}{indent}");
    let i3 = format!("{indent}{indent}{indent}");
    let mut s = String::new();

    let _ = writeln!(s);
    let _ = writeln!(s, "# --- Crabby save-slot helpers ---");

    // Module-level state. Initialized by _rtv_init_save_slot at boot;
    // defaulted here so an early read (before _ready) still resolves
    // somewhere sane.
    let _ = writeln!(s, "var _rtv_save_profile: String = \"default\"");
    let _ = writeln!(s, "var _rtv_save_slot: String = \"default\"");
    let _ = writeln!(s);

    // Path helpers, every rewritten save site funnels through these.
    // Slot dirs live under saves/<profile>/<slot>/ so per-profile slots
    // can share names without colliding.
    let _ = writeln!(s, "func _rtv_save_dir() -> String:");
    let _ = writeln!(s, "{i1}return \"user://saves/\" + _rtv_save_profile + \"/\" + _rtv_save_slot + \"/\"");
    let _ = writeln!(s);
    let _ = writeln!(s, "func _rtv_save_path(name: String) -> String:");
    let _ = writeln!(s, "{i1}return _rtv_save_dir() + name");
    let _ = writeln!(s);

    // Public-facing aliases mods are expected to call through (via
    // Loader autoload, `Loader.save_path(\"my_mod_state.cfg\")`).
    let _ = writeln!(s, "## Public: bare slot name (e.g. \"default\").");
    let _ = writeln!(s, "func active_slot() -> String:");
    let _ = writeln!(s, "{i1}return _rtv_save_slot");
    let _ = writeln!(s);
    let _ = writeln!(s, "## Public: bare profile name owning the active slot.");
    let _ = writeln!(s, "func active_profile() -> String:");
    let _ = writeln!(s, "{i1}return _rtv_save_profile");
    let _ = writeln!(s);
    let _ = writeln!(s, "## Public: absolute save-dir path for the active (profile, slot)");
    let _ = writeln!(s, "## pair, with trailing slash (e.g. `user://saves/default/default/`).");
    let _ = writeln!(s, "func save_dir() -> String:");
    let _ = writeln!(s, "{i1}return _rtv_save_dir()");
    let _ = writeln!(s);
    let _ = writeln!(s, "## Public: resolve a per-slot file path. Mods MUST go through");
    let _ = writeln!(s, "## this for any state that should snapshot/swap with the slot.");
    let _ = writeln!(s, "func save_path(name: String) -> String:");
    let _ = writeln!(s, "{i1}return _rtv_save_path(name)");
    let _ = writeln!(s);

    // Boot init: read active slot, validate, mkdir, migrate legacy.
    let _ = writeln!(s, "func _rtv_init_save_slot() -> void:");
    let _ = writeln!(s, "{i1}var profile: String = \"default\"");
    let _ = writeln!(s, "{i1}var slot: String = \"default\"");
    let _ = writeln!(s, "{i1}if FileAccess.file_exists(\"user://active_slot.txt\"):");
    let _ = writeln!(s, "{i2}var f := FileAccess.open(\"user://active_slot.txt\", FileAccess.READ)");
    let _ = writeln!(s, "{i2}if f != null:");
    let _ = writeln!(s, "{i3}var raw := f.get_as_text()");
    let _ = writeln!(s, "{i3}f.close()");
    let _ = writeln!(s, "{i3}for line in raw.split(\"\\n\"):");
    let _ = writeln!(s, "{i3}{i1}line = line.strip_edges()");
    let _ = writeln!(s, "{i3}{i1}if line.is_empty() or line.begins_with(\"#\"):");
    let _ = writeln!(s, "{i3}{i2}continue");
    let _ = writeln!(s, "{i3}{i1}var eq_at := line.find(\"=\")");
    let _ = writeln!(s, "{i3}{i1}if eq_at < 0:");
    let _ = writeln!(s, "{i3}{i2}continue");
    let _ = writeln!(s, "{i3}{i1}var key := line.substr(0, eq_at).strip_edges()");
    let _ = writeln!(s, "{i3}{i1}var value := line.substr(eq_at + 1).strip_edges()");
    let _ = writeln!(s, "{i3}{i1}if not _rtv_is_safe_slot_name(value):");
    let _ = writeln!(s, "{i3}{i2}push_warning(\"[crabby] active_slot.txt: ignoring unsafe \" + key + \"='\" + value + \"'\")");
    let _ = writeln!(s, "{i3}{i2}continue");
    let _ = writeln!(s, "{i3}{i1}if key == \"profile\":");
    let _ = writeln!(s, "{i3}{i2}profile = value");
    let _ = writeln!(s, "{i3}{i1}elif key == \"slot\":");
    let _ = writeln!(s, "{i3}{i2}slot = value");
    let _ = writeln!(s, "{i1}_rtv_save_profile = profile");
    let _ = writeln!(s, "{i1}_rtv_save_slot = slot");
    let _ = writeln!(s);
    // Flat-slot migration (pre-profile crabby layout) FIRST, must run
    // before make_dir_recursive, otherwise pre-creating the destination
    // causes the flat-slot rename to skip (silently stranding the
    // user's pre-rewrite saves at the wrong path).
    //
    // The vanilla / loose-root migration (`user://*.tres` -> active
    // slot) used to live here too but was removed: the launcher's
    // Saves tab now detects loose vanilla saves and lets the user
    // import them into a profile of their choice (rather than silently
    // claiming them under whatever slot happens to be active).
    let _ = writeln!(s, "{i1}_rtv_migrate_flat_slot_dirs()");
    let _ = writeln!(s);
    let _ = writeln!(s, "{i1}# Ensure the slot dir exists. make_dir_recursive is no-op");
    let _ = writeln!(s, "{i1}# when the path already exists, safe to run every boot.");
    let _ = writeln!(s, "{i1}var dir := DirAccess.open(\"user://\")");
    let _ = writeln!(s, "{i1}if dir != null:");
    let _ = writeln!(s, "{i2}dir.make_dir_recursive(\"saves/\" + _rtv_save_profile + \"/\" + _rtv_save_slot)");
    let _ = writeln!(s);
    let _ = writeln!(s, "{i1}print(\"[crabby] active save: \" + _rtv_save_profile + \"/\" + _rtv_save_slot)");
    let _ = writeln!(s);

    // Name safety: applied to both profile and slot, same rules.
    let _ = writeln!(s, "func _rtv_is_safe_slot_name(name: String) -> bool:");
    let _ = writeln!(s, "{i1}if name.is_empty() or name.length() > 64:");
    let _ = writeln!(s, "{i2}return false");
    let _ = writeln!(s, "{i1}if name.contains(\"/\") or name.contains(\"\\\\\") or name.contains(\"..\"):");
    let _ = writeln!(s, "{i2}return false");
    let _ = writeln!(s, "{i1}for c in name:");
    let _ = writeln!(s, "{i2}var ok := (c >= \"a\" and c <= \"z\") or (c >= \"A\" and c <= \"Z\") or (c >= \"0\" and c <= \"9\") or c == \"-\" or c == \"_\" or c == \" \"");
    let _ = writeln!(s, "{i2}if not ok:");
    let _ = writeln!(s, "{i3}return false");
    let _ = writeln!(s, "{i1}return true");
    let _ = writeln!(s);

    // Migration B: flat slots from the pre-profile rewrite layout.
    // Pre-profile crabby placed slots at saves/<slot>/; profile-aware
    // crabby moves them to saves/<active_profile>/<slot>/. Skipped
    // when the profile dir already has slots (don't shadow).
    let _ = writeln!(s, "func _rtv_migrate_flat_slot_dirs() -> void:");
    let _ = writeln!(s, "{i1}var saves_d := DirAccess.open(\"user://saves\")");
    let _ = writeln!(s, "{i1}if saves_d == null:");
    let _ = writeln!(s, "{i2}return");
    let _ = writeln!(s, "{i1}# Snapshot dir entries first; the walk mutates them.");
    let _ = writeln!(s, "{i1}saves_d.list_dir_begin()");
    let _ = writeln!(s, "{i1}var entries: Array[String] = []");
    let _ = writeln!(s, "{i1}var name := saves_d.get_next()");
    let _ = writeln!(s, "{i1}while name != \"\":");
    let _ = writeln!(s, "{i2}if saves_d.current_is_dir() and not name.begins_with(\".\"):");
    let _ = writeln!(s, "{i3}entries.append(name)");
    let _ = writeln!(s, "{i2}name = saves_d.get_next()");
    let _ = writeln!(s, "{i1}saves_d.list_dir_end()");
    let _ = writeln!(s);
    let _ = writeln!(s, "{i1}# A flat-layout dir is one whose immediate children are the");
    let _ = writeln!(s, "{i1}# slot's save files (e.g. Character.tres). A profile dir's");
    let _ = writeln!(s, "{i1}# immediate children are slot subdirs. Use the file-vs-dir");
    let _ = writeln!(s, "{i1}# heuristic on each entry's first .tres / first child.");
    let _ = writeln!(s, "{i1}for entry in entries:");
    let _ = writeln!(s, "{i2}if not _rtv_is_safe_slot_name(entry):");
    let _ = writeln!(s, "{i3}continue");
    let _ = writeln!(s, "{i2}if entry == _rtv_save_profile:");
    let _ = writeln!(s, "{i3}continue  # already a profile dir");
    let _ = writeln!(s, "{i2}var sub := DirAccess.open(\"user://saves/\" + entry)");
    let _ = writeln!(s, "{i2}if sub == null:");
    let _ = writeln!(s, "{i3}continue");
    let _ = writeln!(s, "{i2}sub.list_dir_begin()");
    let _ = writeln!(s, "{i2}var has_tres := false");
    let _ = writeln!(s, "{i2}var child := sub.get_next()");
    let _ = writeln!(s, "{i2}while child != \"\":");
    let _ = writeln!(s, "{i3}if child.ends_with(\".tres\"):");
    let _ = writeln!(s, "{i3}{i1}has_tres = true");
    let _ = writeln!(s, "{i3}{i1}break");
    let _ = writeln!(s, "{i3}child = sub.get_next()");
    let _ = writeln!(s, "{i2}sub.list_dir_end()");
    let _ = writeln!(s, "{i2}if not has_tres:");
    let _ = writeln!(s, "{i3}continue  # likely a profile dir, not a flat slot");
    let _ = writeln!(s);
    let _ = writeln!(s, "{i2}var dst_parent := \"user://saves/\" + _rtv_save_profile");
    let _ = writeln!(s, "{i2}saves_d.make_dir_recursive(_rtv_save_profile)");
    let _ = writeln!(s, "{i2}var dst := dst_parent + \"/\" + entry");
    let _ = writeln!(s);
    let _ = writeln!(s, "{i2}# Conflict policy: a *populated* dst is a real slot we mustn't");
    let _ = writeln!(s, "{i2}# overwrite. An *empty* dst is a stub (e.g. make_dir_recursive");
    let _ = writeln!(s, "{i2}# pre-created it for the active slot), safe to merge into.");
    let _ = writeln!(s, "{i2}var dst_has_tres := _rtv_dir_has_tres(dst)");
    let _ = writeln!(s, "{i2}if dst_has_tres:");
    let _ = writeln!(s, "{i3}push_warning(\"[crabby] flat-slot migrate: '\" + dst + \"' already populated; leaving '\" + entry + \"' in place\")");
    let _ = writeln!(s, "{i3}continue");
    let _ = writeln!(s);
    let _ = writeln!(s, "{i2}# If dst exists but is empty, copy files in then remove the");
    let _ = writeln!(s, "{i2}# source. (rename() refuses to overwrite an existing dir.)");
    let _ = writeln!(s, "{i2}if DirAccess.dir_exists_absolute(dst):");
    let _ = writeln!(s, "{i3}var moved := _rtv_move_dir_contents(\"user://saves/\" + entry, dst)");
    let _ = writeln!(s, "{i3}if moved >= 0:");
    let _ = writeln!(s, "{i3}{i1}DirAccess.remove_absolute(\"user://saves/\" + entry)");
    let _ = writeln!(s, "{i3}{i1}print(\"[crabby] flat-slot migrate: merged \" + str(moved) + \" file(s) from '\" + entry + \"' into '\" + _rtv_save_profile + \"/\" + entry + \"'\")");
    let _ = writeln!(s, "{i3}else:");
    let _ = writeln!(s, "{i3}{i1}push_warning(\"[crabby] flat-slot migrate: merge failed for '\" + entry + \"'\")");
    let _ = writeln!(s, "{i3}continue");
    let _ = writeln!(s);
    let _ = writeln!(s, "{i2}# Dst doesn't exist, straight rename.");
    let _ = writeln!(s, "{i2}if saves_d.rename(entry, _rtv_save_profile + \"/\" + entry) == OK:");
    let _ = writeln!(s, "{i3}print(\"[crabby] flat-slot migrate: '\" + entry + \"' -> '\" + _rtv_save_profile + \"/\" + entry + \"'\")");
    let _ = writeln!(s, "{i2}else:");
    let _ = writeln!(s, "{i3}push_warning(\"[crabby] flat-slot migrate: rename failed for '\" + entry + \"'\")");
    let _ = writeln!(s);

    // Helper used by the merge path.
    let _ = writeln!(s, "func _rtv_dir_has_tres(path: String) -> bool:");
    let _ = writeln!(s, "{i1}var d := DirAccess.open(path)");
    let _ = writeln!(s, "{i1}if d == null:");
    let _ = writeln!(s, "{i2}return false");
    let _ = writeln!(s, "{i1}d.list_dir_begin()");
    let _ = writeln!(s, "{i1}var n := d.get_next()");
    let _ = writeln!(s, "{i1}while n != \"\":");
    let _ = writeln!(s, "{i2}if n.ends_with(\".tres\"):");
    let _ = writeln!(s, "{i3}d.list_dir_end()");
    let _ = writeln!(s, "{i3}return true");
    let _ = writeln!(s, "{i2}n = d.get_next()");
    let _ = writeln!(s, "{i1}d.list_dir_end()");
    let _ = writeln!(s, "{i1}return false");
    let _ = writeln!(s);
    let _ = writeln!(s, "## Move every loose file from `src_dir` into `dst_dir`. Returns");
    let _ = writeln!(s, "## the count moved, or -1 on error. Skips subdirs (slot dirs");
    let _ = writeln!(s, "## are flat anyway) and the .snapshots subdir. The caller is");
    let _ = writeln!(s, "## responsible for removing src_dir after this returns >= 0.");
    let _ = writeln!(s, "func _rtv_move_dir_contents(src_dir: String, dst_dir: String) -> int:");
    let _ = writeln!(s, "{i1}var sd := DirAccess.open(src_dir)");
    let _ = writeln!(s, "{i1}if sd == null:");
    let _ = writeln!(s, "{i2}return -1");
    let _ = writeln!(s, "{i1}sd.list_dir_begin()");
    let _ = writeln!(s, "{i1}var moved := 0");
    let _ = writeln!(s, "{i1}var n := sd.get_next()");
    let _ = writeln!(s, "{i1}while n != \"\":");
    let _ = writeln!(s, "{i2}if not sd.current_is_dir():");
    let _ = writeln!(s, "{i3}var src := src_dir.trim_suffix(\"/\") + \"/\" + n");
    let _ = writeln!(s, "{i3}var dst := dst_dir.trim_suffix(\"/\") + \"/\" + n");
    let _ = writeln!(s, "{i3}if sd.rename_absolute(src, dst) == OK:");
    let _ = writeln!(s, "{i3}{i1}moved += 1");
    let _ = writeln!(s, "{i3}else:");
    let _ = writeln!(s, "{i3}{i1}push_warning(\"[crabby] move_dir_contents: failed \" + src + \" -> \" + dst)");
    let _ = writeln!(s, "{i2}n = sd.get_next()");
    let _ = writeln!(s, "{i1}sd.list_dir_end()");
    let _ = writeln!(s, "{i1}return moved");

    s
}

/// Find the line index of the first top-level `func` declaration. The
/// dict declarations are injected just before this so they're at module
/// scope but after vanilla's vars/consts/onready decls.
///
/// If no `func` is found (degenerate Loader.gd), insert at end.
fn first_func_line(lines: &[String]) -> usize {
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("func ") || trimmed.starts_with("static func ") {
            return i;
        }
    }
    lines.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_for_non_loader_files() {
        let src = "extends Node\n\nfunc foo(): pass\n";
        let out = transform("NotLoader.gd", src, "\t");
        assert_eq!(out, src);
    }

    #[test]
    fn rewrites_const_shelters_to_var() {
        let src = "extends CanvasLayer\n\nconst shelters = [\"Cabin\", \"Attic\"]\n\nfunc LoadScene(scene: String):\n\tpass\n";
        let out = transform("Loader.gd", src, "\t");
        assert!(
            out.contains("var _rtv_vanilla_shelters: Array = [\"Cabin\", \"Attic\"]"),
            "{out}",
        );
        assert!(
            out.contains("var shelters: Array = _rtv_vanilla_shelters.duplicate()"),
            "{out}",
        );
        assert!(
            !out.contains("const shelters"),
            "const shelters should be gone:\n{out}",
        );
    }

    #[test]
    fn injects_dicts_before_first_func() {
        let src = "extends CanvasLayer\nvar x = 1\nconst shelters = [\"Cabin\"]\n\nfunc LoadScene(scene: String):\n\tpass\n";
        let out = transform("Loader.gd", src, "\t");
        let dict_pos = out.find("_rtv_mod_scene_paths").expect("dict present");
        let func_pos = out.find("func LoadScene").expect("func present");
        assert!(dict_pos < func_pos, "dicts must precede func\n{out}");
    }

    #[test]
    fn injects_load_scene_prelude() {
        let src = "extends CanvasLayer\n\nfunc LoadScene(scene: String):\n\tFadeInLoading()\n";
        let out = transform("Loader.gd", src, "\t");
        assert!(
            out.contains("_rtv_resolved: Dictionary = _rtv_resolve_scene_path(scene)"),
            "{out}",
        );
        assert!(
            out.contains("scenePath = _rtv_resolved.get(\"path\", \"\")"),
            "{out}",
        );
    }

    #[test]
    fn appends_resolver_helper() {
        let src = "extends CanvasLayer\n\nfunc LoadScene(scene: String):\n\tpass\n";
        let out = transform("Loader.gd", src, "\t");
        assert!(out.contains("func _rtv_resolve_scene_path(scene: String)"), "{out}");
        assert!(out.contains("return _rtv_override_scene_paths[scene]"), "{out}");
    }

    #[test]
    fn load_scene_prelude_must_fall_through_not_return() {
        // Regression guard: an earlier version of this prelude returned
        // early after setting scenePath. That short-circuited vanilla's
        // tail which actually calls `change_scene_to_file(scenePath)`,
        // mod scenes never loaded, the game hung on the loading screen.
        // Vanilla's if-elif harmlessly skips mod-named scenes; falling
        // through is the correct shape.
        let src = "extends CanvasLayer\n\nfunc LoadScene(scene: String):\n\tFadeInLoading()\n";
        let out = transform("Loader.gd", src, "\t");
        // Locate the prelude block and check no `return` appears inside it.
        let prelude_start = out
            .find("var _rtv_resolved: Dictionary")
            .expect("prelude present");
        // The prelude ends at the "Fall through" comment (or the next
        // top-level construct, whichever comes first).
        let prelude_end = out[prelude_start..]
            .find("Fall through")
            .map(|i| prelude_start + i)
            .expect("fall-through comment present");
        let prelude_text = &out[prelude_start..prelude_end];
        assert!(
            !prelude_text.contains("return"),
            "prelude must not return early, it would skip change_scene_to_file:\n{prelude_text}",
        );
    }

    #[test]
    fn rewrites_per_slot_save_paths() {
        let src = "extends CanvasLayer\nfunc SaveCharacter():\n\tResourceSaver.save(c, \"user://Character.tres\")\n\tResourceSaver.save(c, \"user://World.tres\")\n\tResourceSaver.save(c, \"user://Cabin.tres\")\n\tResourceSaver.save(c, \"user://Tent.tres\")\n\tResourceSaver.save(c, \"user://Traders.tres\")\n";
        let out = transform("Loader.gd", src, "\t");
        for f in ["Character", "World", "Cabin", "Tent", "Traders"] {
            let needle = format!("_rtv_save_path(\"{f}.tres\")");
            assert!(out.contains(&needle), "expected `{needle}` in output:\n{out}");
            let stale = format!("\"user://{f}.tres\"");
            assert!(!out.contains(&stale), "stale path `{stale}` should be gone:\n{out}");
        }
    }

    #[test]
    fn preserves_install_global_paths() {
        // Validator + Preferences MUST stay at user:// root, they're
        // shared across save slots.
        let src = "extends CanvasLayer\nfunc CreateValidator():\n\tResourceSaver.save(v, \"user://Validator.tres\")\n\tResourceSaver.save(p, \"user://Preferences.tres\")\n";
        let out = transform("Loader.gd", src, "\t");
        assert!(out.contains("\"user://Validator.tres\""), "Validator must stay global:\n{out}");
        assert!(out.contains("\"user://Preferences.tres\""), "Preferences must stay global:\n{out}");
    }

    #[test]
    fn rewrites_dynamic_shelter_path() {
        let src = "extends CanvasLayer\nfunc SaveShelter(targetShelter):\n\tResourceSaver.save(s, \"user://\" + targetShelter + \".tres\")\n";
        let out = transform("Loader.gd", src, "\t");
        assert!(
            out.contains("_rtv_save_path(targetShelter + \".tres\")"),
            "dynamic shelter path must funnel through helper:\n{out}",
        );
        assert!(
            !out.contains("\"user://\" + targetShelter"),
            "stale dynamic path should be gone:\n{out}",
        );
    }

    #[test]
    fn rewrites_diraccess_open_to_slot_dir() {
        let src = "extends CanvasLayer\nfunc FormatSave():\n\tvar directory = DirAccess.open(\"user://\")\n\tvar p = \"user://\" + file\n";
        let out = transform("Loader.gd", src, "\t");
        assert!(
            out.contains("DirAccess.open(_rtv_save_dir())"),
            "DirAccess scan must target slot dir:\n{out}",
        );
        assert!(
            out.contains("_rtv_save_dir() + file"),
            "loose `\"user://\" + file` concat must be slot-aware:\n{out}",
        );
    }

    #[test]
    fn injects_init_call_into_ready() {
        let src = "extends CanvasLayer\nfunc _ready():\n\tmasterAmplify.volume_db = linear_to_db(0)\n";
        let out = transform("Loader.gd", src, "\t");
        // The init call must land BEFORE the existing _ready body so
        // _rtv_save_slot is populated by the time anything else needs
        // a save path.
        let init_pos = out.find("_rtv_init_save_slot()").expect("init call present");
        let body_pos = out.find("masterAmplify.volume_db").expect("body present");
        assert!(
            init_pos < body_pos,
            "_rtv_init_save_slot must run before vanilla _ready body:\n{out}",
        );
    }

    #[test]
    fn appends_save_slot_helpers() {
        let src = "extends CanvasLayer\nfunc _ready():\n\tpass\n";
        let out = transform("Loader.gd", src, "\t");
        for needle in [
            "func _rtv_save_dir() -> String:",
            "func _rtv_save_path(name: String) -> String:",
            "func _rtv_init_save_slot() -> void:",
            "func _rtv_is_safe_slot_name(name: String) -> bool:",
            "func _rtv_migrate_flat_slot_dirs() -> void:",
            "var _rtv_save_profile: String = \"default\"",
            "var _rtv_save_slot: String = \"default\"",
        ] {
            assert!(out.contains(needle), "missing helper `{needle}`:\n{out}");
        }
    }

    #[test]
    fn no_in_game_loose_root_save_migration() {
        // The launcher owns vanilla-save handling now -- the rewriter
        // must NOT silently migrate loose root .tres files into the
        // active slot. Regression guard: if anyone re-introduces this,
        // the launcher's import-to-profile flow becomes a race.
        let src = "extends CanvasLayer\nfunc _ready():\n\tpass\n";
        let out = transform("Loader.gd", src, "\t");
        assert!(
            !out.contains("_rtv_migrate_legacy_saves"),
            "legacy-save migration must not be re-introduced into the rewriter:\n{out}",
        );
    }

    #[test]
    fn save_dir_includes_profile() {
        // Slot path must be saves/<profile>/<slot>/, not the old
        // saves/<slot>/. The profile-aware layout is the contract.
        let src = "extends CanvasLayer\nfunc _ready():\n\tpass\n";
        let out = transform("Loader.gd", src, "\t");
        assert!(
            out.contains("\"user://saves/\" + _rtv_save_profile + \"/\" + _rtv_save_slot + \"/\""),
            "save_dir must concat profile + slot:\n{out}",
        );
    }

    #[test]
    fn appends_public_save_path_api() {
        // Public wrappers mods are expected to call through. Renaming
        // any of these would break the supported save API.
        let src = "extends CanvasLayer\nfunc _ready():\n\tpass\n";
        let out = transform("Loader.gd", src, "\t");
        for needle in [
            "func active_slot() -> String:",
            "func save_dir() -> String:",
            "func save_path(name: String) -> String:",
        ] {
            assert!(out.contains(needle), "missing public API `{needle}`:\n{out}");
        }
    }
}
