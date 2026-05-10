//! Sweep both the dev mod source dir AND the installed `<game-dir>/Mods/`
//! through the analyzer. Two purposes:
//!
//! 1. **Coverage**, surface false positives / negatives across many
//!    real mods, not just TraderImprovements.
//! 2. **Source ↔ archive parity**, for mods present in both roots,
//!    do the analyzer findings match? A divergence means vmz packing
//!    drops/transforms something the dev source contains, or vice
//!    versa.
//!
//! Both roots are absolute paths gated on existence, with no-op if
//! either is missing (CI, fresh clones).

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use crabby_mod_analyzer::{analyze_mod, ModIntent};

const DEV_ROOT: &str = "/mnt/c/Users/ashou/GitHub/thc/Road to Vostok/Mods";
const GAME_ROOT: &str = "/mnt/e/SteamLibrary/steamapps/common/Road to Vostok/Mods";

#[test]
fn all_mods_sweep() {
    let dev = Path::new(DEV_ROOT);
    let game = Path::new(GAME_ROOT);
    if !dev.exists() && !game.exists() {
        eprintln!("skipping: neither {DEV_ROOT} nor {GAME_ROOT} present");
        return;
    }

    let mut dev_results: BTreeMap<String, Summary> = BTreeMap::new();
    let mut game_results: BTreeMap<String, Summary> = BTreeMap::new();

    if dev.exists() {
        eprintln!("\n=== DEV ROOT: {DEV_ROOT} ===");
        for entry in std::fs::read_dir(dev).expect("read dev root") {
            let entry = entry.expect("entry");
            let path = entry.path();
            // Folders only (dev source); skip the analyzer crate's own
            // workspace, vostok-mod-loader src (we're not analyzing the
            // loader itself).
            if !path.is_dir() {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap().to_string();
            // Skip non-mod dirs (crabby itself, etc.).
            if name.starts_with('.') || name == "vostok-mod-loader" || name == "crabby-loader" {
                continue;
            }
            let files = read_folder_mod(&path);
            if files.is_empty() {
                continue;
            }
            let intent = analyze_mod(&name, files.iter().map(|(n, s)| (n.as_str(), s.as_str())));
            let summary = Summary::from(&intent);
            print_summary(&name, &summary);
            dev_results.insert(name, summary);
        }
    }

    if game.exists() {
        eprintln!("\n=== INSTALLED ROOT: {GAME_ROOT} ===");
        for entry in std::fs::read_dir(game).expect("read game root") {
            let entry = entry.expect("entry");
            let path = entry.path();
            // Archives only.
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext != "vmz" && ext != "zip" {
                continue;
            }
            let stem = path.file_stem().and_then(|n| n.to_str()).unwrap().to_string();
            let files = match read_archive_mod(&path) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("{stem}: archive read failed: {e}");
                    continue;
                }
            };
            if files.is_empty() {
                continue;
            }
            let intent = analyze_mod(&stem, files.iter().map(|(n, s)| (n.as_str(), s.as_str())));
            let summary = Summary::from(&intent);
            print_summary(&stem, &summary);
            game_results.insert(stem, summary);
        }
    }

    // Source ↔ archive parity. For every mod that appears in both
    // roots (matched by case-insensitive substring on the name, since
    // archive names sometimes mangle spaces and casing), compare.
    eprintln!("\n=== SOURCE ↔ ARCHIVE PARITY ===");
    let mut compared = 0usize;
    let mut diverged = 0usize;
    for (dev_name, dev_sum) in &dev_results {
        let dev_key = dev_name.to_ascii_lowercase().replace(['_', ' ', '-'], "");
        let Some((game_name, game_sum)) = game_results.iter().find(|(g, _)| {
            let g_key = g.to_ascii_lowercase().replace(['_', ' ', '-'], "");
            g_key.contains(&dev_key) || dev_key.contains(&g_key)
        }) else {
            continue;
        };
        compared += 1;
        if dev_sum != game_sum {
            diverged += 1;
            eprintln!(
                "DIVERGE  dev=`{dev_name}` archive=`{game_name}`\n  dev   : {dev_sum:?}\n  archive: {game_sum:?}",
            );
        } else {
            eprintln!("MATCH    dev=`{dev_name}` archive=`{game_name}`  {dev_sum:?}");
        }
    }
    eprintln!(
        "\nparity: {compared} comparable mod(s), {diverged} divergence(s)",
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Summary {
    files: usize,
    hooks: usize,
    static_hooks: usize,
    registry_writes: usize,
    classic_hard: usize,
    classic_warn: usize,
    classic_info: usize,
}

impl From<&ModIntent> for Summary {
    fn from(i: &ModIntent) -> Self {
        use crabby_mod_analyzer::{Resolvability, Severity};
        let static_hooks = i
            .hooks
            .iter()
            .filter(|h| h.resolvability == Resolvability::Static)
            .count();
        let mut hard = 0;
        let mut warn = 0;
        let mut info = 0;
        for c in &i.classic_patterns {
            match c.severity {
                Severity::Hard => hard += 1,
                Severity::Warn => warn += 1,
                Severity::Info => info += 1,
            }
        }
        Self {
            files: i.files_scanned.len(),
            hooks: i.hooks.len(),
            static_hooks,
            registry_writes: i.registry_writes.len(),
            classic_hard: hard,
            classic_warn: warn,
            classic_info: info,
        }
    }
}

fn print_summary(name: &str, s: &Summary) {
    eprintln!(
        "  {name:50} files={:3}  hooks={:3} (static {:3})  reg={:3}  classic H/W/I={}/{}/{}",
        s.files, s.hooks, s.static_hooks, s.registry_writes, s.classic_hard, s.classic_warn, s.classic_info,
    );
}

/// Recursively read every `.gd` file under `dir`, returning
/// `(relative_filename, source)` pairs.
fn read_folder_mod(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        // Skip dot-dirs (.git etc).
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with('.'))
            .unwrap_or(false)
        {
            continue;
        }
        if path.is_dir() {
            walk(root, &path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("gd") {
            let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned();
            if let Ok(s) = std::fs::read_to_string(&path) {
                out.push((rel, s));
            }
        }
    }
}

/// Open a `.vmz`/`.zip` and pull every `.gd` entry. Filenames are
/// the in-archive paths.
fn read_archive_mod(path: &PathBuf) -> Result<Vec<(String, String)>, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("zip: {e}"))?;
    let mut out = Vec::new();
    for i in 0..z.len() {
        let mut entry = z.by_index(i).map_err(|e| format!("entry {i}: {e}"))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        if !name.ends_with(".gd") {
            continue;
        }
        let mut buf = String::new();
        if entry.read_to_string(&mut buf).is_err() {
            // Non-utf8, skip (rare for .gd, but not impossible if the
            // packer included weird files).
            continue;
        }
        out.push((name, buf));
    }
    Ok(out)
}
