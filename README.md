# crabby-loader

Mod loader for Road to Vostok. Bakes a modified `RTV.pck` so installed
mods load when the game starts. Coexists with [vostok-mod-loader](../vostok-mod-loader/);
pick whichever you prefer.

Status: alpha. Use at your own risk; back up your save dir before installing.

## What it does

- Rewrites the vanilla `RTV.pck` in place with hook + registry scaffolding,
  so mods can patch any vanilla script or registry entry without touching
  game code.
- Ships a runtime API (hooks, registries, save-aware paths) baked into
  the PCK. Vostok mods drop in unmodified.
- Provides a desktop launcher for installing, managing profiles, toggling
  mods, and snapshotting saves.

## Install

1. Grab the latest `crabby` binary from releases (Windows: `crabby.exe`,
   Linux: `crabby`).
2. Run it. First launch auto-detects your Steam install or prompts for
   the game directory (the folder containing `RTV.pck`).
3. Drop mods into `<game-dir>/Mods/`. Supported formats: `.vmz`, `.zip`,
   or unpacked folders with a `mod.txt` at the root.
4. Hit **Install / Re-bake** in the launcher. This rewrites `RTV.pck`;
   vanilla bytes are preserved at `RTV.pck.vanilla.bak` for uninstall.
5. Hit **Launch game** to play, or launch through Steam normally.

The bake takes around 10-15s on an SSD. Re-bake any time you toggle mods
or switch profiles.

## Uninstall

Delete `RTV.pck`, rename `RTV.pck.vanilla.bak` back to `RTV.pck`, and
delete the `.crabby/` directory in your game folder. Or run `Verify
integrity of game files` in Steam.

## Build from source

```sh
cargo build --release -p crabby-ui
```

Binary lands at `target/release/crabby` (or `crabby.exe` on Windows).

Requires Rust 1.94+ (2024 edition); pinned via `rust-toolchain.toml`.

## Layout

- `crates/` - Rust workspace (bake pipeline, launcher UI, CLI helpers)
- `shim/` - GDScript runtime API, baked into the PCK at install time
- `tests/` - fixtures and differential tests
