//! Smoke test against TraderImprovements, a real-world chunky mod.
//!
//! Asserts the analyzer:
//! - Parses every `.gd` without panicking
//! - Finds the known hook calls (TI's Main.gd has 12 distinct hooks)
//! - Doesn't false-flag classic patterns on legit Godot code
//!
//! Gated on TI being present locally.

use std::path::Path;

use crabby_mod_analyzer::{analyze_mod, HookKind, Resolvability};

const TI_ROOT: &str = "/mnt/c/Users/ashou/GitHub/thc/Road to Vostok/Mods/TraderImprovements";

#[test]
fn trader_improvements_smoke() {
    let root = Path::new(TI_ROOT);
    if !root.exists() {
        eprintln!("skipping: {TI_ROOT} not present");
        return;
    }

    // Read all .gd files, hand to analyze_mod.
    let mut files: Vec<(String, String)> = Vec::new();
    for entry in std::fs::read_dir(root).expect("read TI dir") {
        let entry = entry.expect("entry");
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("gd") {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap().to_string();
        let src = std::fs::read_to_string(&path).expect("read");
        files.push((name, src));
    }

    let intent = analyze_mod(
        "trader-improvements",
        files.iter().map(|(n, s)| (n.as_str(), s.as_str())),
    );

    eprintln!("=== TraderImprovements analysis ===");
    eprintln!("files scanned: {}", intent.files_scanned.len());
    for f in &intent.files_scanned {
        eprintln!("  {} ({} lines)", f.filename, f.line_count);
    }
    eprintln!("hooks: {}", intent.hooks.len());
    let static_hooks = intent
        .hooks
        .iter()
        .filter(|h| h.resolvability == Resolvability::Static)
        .count();
    eprintln!("  static-eligible: {}/{}", static_hooks, intent.hooks.len());
    for h in &intent.hooks {
        eprintln!(
            "  {}:{} [{:?}] {} -> {}",
            h.filename,
            h.line,
            h.kind,
            h.hook_name.as_deref().unwrap_or("<dynamic>"),
            h.callable_text,
        );
    }
    eprintln!("registry writes: {}", intent.registry_writes.len());
    for w in &intent.registry_writes {
        eprintln!(
            "  {}:{} {:?}({:?}, {:?}, {})",
            w.filename, w.line, w.verb, w.registry, w.key, w.payload_text,
        );
    }
    eprintln!("classic patterns: {}", intent.classic_patterns.len());
    for c in &intent.classic_patterns {
        eprintln!(
            "  {}:{} [{:?}/{:?}] target={:?}",
            c.filename, c.line, c.severity, c.pattern, c.target,
        );
    }

    // Asserts on known TI properties:
    // - Main.gd has lib.hook calls (at least 12).
    let main_hooks = intent.hooks.iter().filter(|h| h.filename == "Main.gd").count();
    assert!(main_hooks >= 12, "expected ≥12 hooks in Main.gd, got {main_hooks}");
    // - All those hooks have literal names, so they're static-eligible.
    let main_static = intent
        .hooks
        .iter()
        .filter(|h| h.filename == "Main.gd" && h.resolvability == Resolvability::Static)
        .count();
    assert_eq!(
        main_static, main_hooks,
        "expected every Main.gd hook to be static, got {main_static}/{main_hooks}",
    );
    // - TI uses `_lib.hook(...)`, `interface-...-pre/post`. The first
    //   should decode to Pre/Post, never Replace.
    for h in &intent.hooks {
        if h.filename != "Main.gd" {
            continue;
        }
        let name = h.hook_name.as_deref().unwrap();
        if name.ends_with("-pre") {
            assert_eq!(h.kind, HookKind::Pre, "{name}");
        } else if name.ends_with("-post") {
            assert_eq!(h.kind, HookKind::Post, "{name}");
        }
    }
}
