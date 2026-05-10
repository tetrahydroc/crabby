//! Install-artifact constants + embedded Lib source.
//!
//! Lib (the modding API + boot orchestrator) is split across
//! `shim/lib/lib.gd` (class header, state, dispatch primitives,
//! public hook + save API), `shim/lib/boot.gd` (the `_ready` + mod-
//! mount machinery, formerly the on-disk `crabby_shim.gd`), and the
//! per-kind files under `shim/registry/`. Each is `include_str!`'d at
//! compile time and concatenated into [`LIB_SOURCE`]. Build cache
//! invalidates when any source file changes.

/// Filename of the generated hook pack.
pub const HOOK_PACK_FILE_NAME: &str = "crabby_hook_pack.zip";

/// Directory (relative to the game dir) that holds crabby state, e.g.
/// install manifest and pack backups. Hidden-prefix so it reads as
/// infrastructure.
pub const MANIFEST_DIR: &str = ".crabby";

/// Filename of the JSON install manifest under [`MANIFEST_DIR`].
pub const MANIFEST_FILE_NAME: &str = "install.json";

/// Filename of the override.cfg backup taken on install when the game dir
/// already had a user-authored override.cfg.
pub const OVERRIDE_CFG_BACKUP_NAME: &str = "override.cfg.crabby.backup";

/// Name of the override.cfg Godot reads at engine startup.
pub const OVERRIDE_CFG_NAME: &str = "override.cfg";

/// Filename of the legacy on-disk shim. Boot orchestration now lives
/// in Lib via the `boot.gd` LIB_FRAGMENTS entry. The name is kept as
/// a constant so the install path can look for and remove orphaned
/// copies left over from older builds.
pub const LEGACY_SHIM_FILE_NAME: &str = "crabby_shim.gd";

/// Filename of the main RTV pack as Godot expects to find it.
pub const VANILLA_PCK_NAME: &str = "RTV.pck";

/// Filename of the vanilla-PCK backup crabby keeps. Lives next to `RTV.pck`
/// so the launcher can find it without consulting the manifest.
pub const VANILLA_PCK_BACKUP_NAME: &str = "RTV.pck.vanilla.bak";

// --- Lib (in-PCK vanilla autoload) ---------------------------------------
//
// `Lib` is the modding API baked directly into vanilla. Unlike the legacy
// `CrabbyShim` autoload (which lives at the game-dir root and is mounted
// via `override.cfg`), `Lib` ships *inside* RTV.pck as a regular vanilla
// script, registered in vanilla's `project.godot` under `[autoload]`. To
// the engine it is indistinguishable from `Database`, `LT_Master`, etc.

/// PCK-internal path where the bake emits `Lib.gd`. Mods reach this via
/// the `Lib` global once the autoload is registered.
pub const LIB_PCK_PATH: &str = "res://Lib.gd";

/// Autoload name registered in vanilla `project.godot` for `Lib`.
pub const LIB_AUTOLOAD_NAME: &str = "Lib";

/// Per-source-file fragments for the Lib autoload. Order is load-bearing,
/// declarations before use.
///
/// `lib.gd` MUST be first since it owns the class declaration (extends
/// Node, signals, state vars, dispatch primitives). Subsequent fragments
/// append methods to the same class.
///
/// `boot.gd` comes second, since it owns the `_ready` orchestrator and
/// the mod-mount machinery. Placed before any registry fragment so the
/// boot pipeline is together with Lib's class header for readability.
///
/// `registry/shared.gd` provides helpers used by every kind, so it
/// comes before the per-kind handlers. `registry/api.gd` defines the
/// public verb dispatch (register/override/patch/append/etc.) that
/// per-kind handlers feed into. `registry/aggregators.gd` is the
/// register_item/_weapon/_magazine/_attachment/_furniture/_ai_loadout
/// surface, depending on api + per-kind handlers being declared.
/// `registry/setup.gd` is the declarative plan dispatcher and depends
/// on every other verb being available, so it comes last.
const LIB_FRAGMENTS: &[(&str, &str)] = &[
    ("lib.gd", include_str!("../../../shim/lib/lib.gd")),
    ("boot.gd", include_str!("../../../shim/lib/boot.gd")),
    (
        "registry/shared.gd",
        include_str!("../../../shim/registry/shared.gd"),
    ),
    (
        "registry/api.gd",
        include_str!("../../../shim/registry/api.gd"),
    ),
    (
        "registry/aggregators.gd",
        include_str!("../../../shim/registry/aggregators.gd"),
    ),
    (
        "registry/ai.gd",
        include_str!("../../../shim/registry/ai.gd"),
    ),
    (
        "registry/ai_loadouts.gd",
        include_str!("../../../shim/registry/ai_loadouts.gd"),
    ),
    (
        "registry/events.gd",
        include_str!("../../../shim/registry/events.gd"),
    ),
    (
        "registry/fish.gd",
        include_str!("../../../shim/registry/fish.gd"),
    ),
    (
        "registry/inputs.gd",
        include_str!("../../../shim/registry/inputs.gd"),
    ),
    (
        "registry/items.gd",
        include_str!("../../../shim/registry/items.gd"),
    ),
    (
        "registry/loader.gd",
        include_str!("../../../shim/registry/loader.gd"),
    ),
    (
        "registry/loot.gd",
        include_str!("../../../shim/registry/loot.gd"),
    ),
    (
        "registry/recipes.gd",
        include_str!("../../../shim/registry/recipes.gd"),
    ),
    (
        "registry/resources.gd",
        include_str!("../../../shim/registry/resources.gd"),
    ),
    (
        "registry/scene_nodes.gd",
        include_str!("../../../shim/registry/scene_nodes.gd"),
    ),
    (
        "registry/scenes.gd",
        include_str!("../../../shim/registry/scenes.gd"),
    ),
    (
        "registry/sounds.gd",
        include_str!("../../../shim/registry/sounds.gd"),
    ),
    (
        "registry/traders.gd",
        include_str!("../../../shim/registry/traders.gd"),
    ),
    (
        "registry/setup.gd",
        include_str!("../../../shim/registry/setup.gd"),
    ),
];

/// Assembled Lib source, lazily built on first access. Concatenates
/// every `LIB_FRAGMENTS` body with a `# ===== <relative_path> =====`
/// banner before each so anyone reading the installed file (e.g. via
/// the `dump_shim` example) can trace any line back to its source.
pub static LIB_SOURCE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let mut out =
        String::with_capacity(LIB_FRAGMENTS.iter().map(|(_, s)| s.len()).sum::<usize>() + 256);
    for (i, (name, body)) in LIB_FRAGMENTS.iter().enumerate() {
        if i > 0 {
            while out.ends_with("\n\n") {
                out.pop();
            }
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
        }
        out.push_str(&format!("# ===== {name} =====\n"));
        out.push_str(body);
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
});
