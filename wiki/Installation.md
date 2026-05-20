# Installation

## Quick start

1. Grab the latest `crabby` binary from
   [Releases](https://github.com/tetrahydroc/crabby/releases) (Windows:
   `crabby-windows-x86_64.zip`, Linux: `crabby-linux-x86_64.tar.gz`).
2. Extract and run. First launch auto-detects your Steam install or
   prompts for the game directory (the folder containing `RTV.pck`).
3. Drop mods into `<game-dir>/mods/`. Or click add file in the ui to install from anywhere
   Supported formats:
   - `.vmz` (the vostok / crabby archive format, preferred)
   - `.zip` with a `mod.txt` at the root
   - unpacked folder with a `mod.txt` at the root
4. Hit **Install / Re-bake** in the launcher. This rewrites `RTV.pck`;
   vanilla bytes are preserved at `RTV.pck.vanilla.bak` for uninstall.
5. Hit **Launch game** to play, or launch through Steam normally.

The bake takes around 10-15 seconds on an SSD. Re-bake any time you
toggle mods or switch profiles.

## Profiles

Profiles let you keep multiple mod configurations side by side. The
profile bar (under the tabs) shows the active profile and its enabled
mod count. Switching profiles writes through to the config; the next
launch reads the new active set. Each bake is keyed to the profile,
so re-baking is required when you switch.

Click **Edit profile** for the inline editor:

- Type a name + **Create** to make a new profile (it becomes active).
- Type a new name + **Rename "<active>"** to rename the active profile.
- Click any non-active profile chip in the **Delete:** row to remove it.
  The active profile and the last remaining profile can't be deleted.

## First-run on Windows

The unsigned executable will trip Windows SmartScreen. Click **More
info** -> **Run anyway** the first time. Subsequent launches skip the
warning. Code signing is on the post-alpha roadmap.

## Uninstall

Two options:

- **Manual**: delete `RTV.pck`, rename `RTV.pck.vanilla.bak` back to
  `RTV.pck`, and delete the `.crabby/` directory in your game folder.
- **Steam**: right-click the game in Steam → Properties → Installed
  Files → Verify integrity. Steam re-downloads `RTV.pck` and removes
  any crabby-introduced files from the manifest's perspective.

Save files are not touched by uninstall

## Build from source

```sh
cargo build --release -p crabby-ui
```

Binary lands at `target/release/crabby` (or `crabby.exe` on Windows).
Requires Rust 1.94+ (2024 edition); the toolchain is pinned via
`rust-toolchain.toml`.
