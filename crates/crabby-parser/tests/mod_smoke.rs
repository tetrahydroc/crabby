//! Smoke test: parse a real-world mod's GDScript files.
//!
//! The static scanner needs `crabby-parser` to handle text-form
//! `.gd` from mods (vmz / folder layout), not just detokenized RTV
//! scripts. This test loads TraderImprovements (the chunkiest known
//! mod) and asserts every file parses without error and surfaces a
//! sane functions/var_names count.
//!
//! Gated on the directory existing; runs locally where the mod is
//! checked out, no-ops in CI / fresh clones.

use std::path::Path;

use crabby_parser::parse_script;

const MOD_ROOT: &str = "/mnt/c/Users/ashou/GitHub/thc/Road to Vostok/Mods/TraderImprovements";

#[test]
fn trader_improvements_parses_clean() {
    let root = Path::new(MOD_ROOT);
    if !root.exists() {
        eprintln!("skipping: {MOD_ROOT} not present");
        return;
    }

    let mut total_files = 0;
    let mut total_funcs = 0;
    let mut total_vars = 0;
    let mut failures: Vec<String> = Vec::new();

    for entry in std::fs::read_dir(root).expect("read mod dir") {
        let entry = entry.expect("entry");
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("gd") {
            continue;
        }
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("file name")
            .to_string();
        let src = std::fs::read_to_string(&path).expect("read gd");
        match parse_script(&filename, &src) {
            Ok(p) => {
                total_files += 1;
                total_funcs += p.functions.len();
                total_vars += p.var_names.len();
                eprintln!(
                    "  {filename}: extends={:?} class_name={:?} funcs={} vars={}",
                    p.extends,
                    p.class_name,
                    p.functions.len(),
                    p.var_names.len(),
                );
            }
            Err(e) => failures.push(format!("{filename}: {e}")),
        }
    }

    eprintln!("parsed {total_files} files, {total_funcs} funcs, {total_vars} vars",);

    if !failures.is_empty() {
        panic!("parse failures:\n  {}", failures.join("\n  "));
    }
    assert!(
        total_files >= 4,
        "expected to parse all 4 mod files, got {total_files}"
    );
}
