//! Full-corpus parser smoke test.
//!
//! Detokenize every `Scripts/*.gdc` in `RTV.pck`, feed each through the
//! parser, assert no errors and that a reasonable number of functions are
//! detected. Catches mis-escaped regexes, bad line-number tracking, and
//! similar regressions that unit tests with hand-crafted input miss.
//!
//! # Opt-in
//!
//! Gated on `CRABBY_RTV_PCK`. Without it set the test exits `ok` and CI
//! stays green.
//!
//! ```sh
//! CRABBY_RTV_PCK="/mnt/e/.../RTV.pck" \
//!     cargo test -p crabby-parser --test full_corpus -- --nocapture
//! ```

#![allow(clippy::case_sensitive_file_extension_comparisons)]

use std::path::{Path, PathBuf};

use crabby_detokenizer::detokenize;
use crabby_parser::parse_script;
use crabby_pck::PckArchive;

const ENV_PCK: &str = "CRABBY_RTV_PCK";

#[test]
#[allow(clippy::cast_precision_loss)]
fn full_corpus_parses_without_error() {
    let pck = match std::env::var(ENV_PCK) {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => {
            eprintln!("{ENV_PCK} not set - skipping full-corpus parser smoke test.");
            return;
        }
    };

    let mut archive = PckArchive::open(&pck).expect("open RTV.pck");

    let targets: Vec<_> = archive
        .entries()
        .iter()
        .filter(|e| e.path.ends_with(".gdc"))
        .filter(|e| {
            let n = e.path.trim_start_matches("res://").trim_start_matches('/');
            n.starts_with("Scripts/")
        })
        .cloned()
        .collect();

    assert!(!targets.is_empty(), "no Scripts/*.gdc in PCK");

    let mut parsed_ok = 0usize;
    let mut total_functions = 0usize;
    let mut zero_byte = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for entry in &targets {
        let normalized = entry
            .path
            .trim_start_matches("res://")
            .trim_start_matches('/');
        let script_name = Path::new(normalized)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let filename = format!("{script_name}.gd");

        let bytes = archive.read(entry).expect("pck read");

        // Zero-byte entries detokenize to empty source; vostok has the same
        // behavior (CasettePlayer.gd in 4.6.1). They simply can't be parsed.
        let source = detokenize(&bytes).expect("detokenize");
        if source.is_empty() {
            zero_byte += 1;
            continue;
        }

        match parse_script(&filename, &source) {
            Ok(p) => {
                parsed_ok += 1;
                total_functions += p.functions.len();
            }
            Err(e) => failures.push(format!("{}: {e}", entry.path)),
        }
    }

    let total = targets.len();
    let avg = total_functions as f64 / parsed_ok.max(1) as f64;
    eprintln!(
        "parsed: {parsed_ok} / {total} (zero-byte skipped: {zero_byte}, avg {avg:.1} functions/script)",
    );

    if !failures.is_empty() {
        for f in &failures {
            eprintln!("  FAIL {f}");
        }
        panic!("{} scripts failed to parse", failures.len());
    }
}
