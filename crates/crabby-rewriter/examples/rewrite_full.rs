//! Small example: detokenize a `.gdc`, run the full-script rewriter,
//! print a summary of what was produced.
//!
//! ```sh
//! cargo run --example rewrite_full -p crabby-rewriter -- /path/to/Controller.gdc
//! # with --dump to also print the rewritten source:
//! cargo run --example rewrite_full -p crabby-rewriter -- /path/to/X.gdc --dump
//! ```

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crabby_detokenizer::detokenize;
use crabby_parser::parse_script;
use crabby_rewriter::rewrite_full_script;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let path: PathBuf = args
        .next()
        .ok_or("usage: rewrite_full <path-to-gdc> [--dump]")?
        .into();
    let dump = args.any(|a| a == "--dump");

    let bytes = fs::read(&path)?;
    let source = detokenize(&bytes)?;
    let filename = Path::new(&path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map_or_else(|| "Unknown.gd".to_string(), |s| format!("{s}.gd"));

    let parsed = parse_script(&filename, &source)?;
    let rewritten = rewrite_full_script(&source, &parsed)?;

    let orig_bytes = source.len();
    let new_bytes = rewritten.len();
    let hookable = parsed.functions.iter().filter(|f| !f.is_static).count();
    let coroutines = parsed
        .functions
        .iter()
        .filter(|f| !f.is_static && f.is_coroutine)
        .count();

    eprintln!("file:              {filename}");
    eprintln!("source bytes:      {orig_bytes}");
    eprintln!("rewritten bytes:   {new_bytes}");
    eprintln!("functions:         {}", parsed.functions.len());
    eprintln!("hookable (wrapped): {hookable}");
    eprintln!("coroutine methods: {coroutines}");
    eprintln!(
        "first wrapped name: {}",
        parsed
            .functions
            .iter()
            .find(|f| !f.is_static)
            .map_or("-", |f| f.name.as_str()),
    );
    // Sanity checks.
    let renamed_count = rewritten.matches("func _rtv_vanilla_").count();
    let dispatch_count = rewritten.matches("_lib._dispatch(").count();
    let await_vanilla_sites = rewritten.matches("await _rtv_vanilla_").count();
    eprintln!("renamed funcs:     {renamed_count}");
    eprintln!(
        "dispatch calls:    {dispatch_count} (expect {} = 2 × hookable)",
        hookable * 2,
    );
    // The void template has 5 vanilla-call sites per wrapper (3 short-circuit
    // branches + replace-no-skip + no-replace fallthrough); non-void adds a
    // 6th site threading `_result`. Fast templates have 3 (short-circuit +
    // replace-no-skip + no-replace).
    eprintln!(
        "await-vanilla sites: {await_vanilla_sites} (void coroutine → 5 per, non-void → 6, fast → 3)",
    );

    if dump {
        print!("{rewritten}");
    }
    Ok(())
}
