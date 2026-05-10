//! Full-corpus differential test: iterate every `.gdc` in `RTV.pck`,
//! detokenize with crabby, and assert two things:
//!
//! 1. **Every** script detokenizes without error. Crabby's coverage
//!    target is the full PCK, vostok's skip lists are vostok's
//!    implementation choice, not a constraint crabby inherits.
//! 2. Where vostok has a cached detokenization, crabby's output is
//!    byte-identical. This catches format drift without requiring a
//!    full ground-truth corpus.
//!
//! # Opt-in
//!
//! This test is machine-specific (needs the real PCK + vostok's detokenize
//! cache). It's gated on two environment variables:
//!
//! - `CRABBY_RTV_PCK`    - absolute path to `RTV.pck`
//! - `CRABBY_VOSTOK_CACHE` - absolute path to `modloader_hooks/vanilla/`
//!   (vostok's detokenize cache, typically
//!   `%APPDATA%/Road to Vostok/modloader_hooks/vanilla/`). Optional; if not
//!   set, the test asserts only #1 (no detokenize errors).
//!
//! Without `CRABBY_RTV_PCK` set, the test exits `ok` with a note so CI
//! stays green. Set the vars locally to run the real differential:
//!
//! ```sh
//! CRABBY_RTV_PCK="/mnt/e/.../RTV.pck" \
//! CRABBY_VOSTOK_CACHE="/mnt/c/.../Road to Vostok/modloader_hooks/vanilla" \
//!     cargo test -p crabby-detokenizer --test full_corpus -- --nocapture
//! ```
//!
//! # Scoring
//!
//! The test never stops at the first divergence; it records every script
//! that diverges and fails with a per-script summary. One run surfaces
//! every missing Variant type / spacing bug / edge case so they can
//! be fixed as a batch rather than one-round-trip-per-failure.

#![allow(clippy::case_sensitive_file_extension_comparisons)]

use std::fs;
use std::path::{Path, PathBuf};

use crabby_detokenizer::detokenize;
use crabby_pck::PckArchive;

const ENV_PCK: &str = "CRABBY_RTV_PCK";
const ENV_CACHE: &str = "CRABBY_VOSTOK_CACHE";

#[derive(Debug)]
enum Divergence {
    /// crabby returned Err
    CrabbyError { message: String },
    /// crabby returned Ok but bytes differed
    ByteMismatch {
        crabby_len: usize,
        vostok_len: usize,
        first_diff_byte: usize,
        crabby_preview: String,
        vostok_preview: String,
    },
}

#[derive(Debug)]
struct Report {
    entry_path: String,
    divergence: Divergence,
}

#[test]
fn full_corpus_matches_vostok() {
    let pck = match std::env::var(ENV_PCK) {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => {
            eprintln!(
                "{ENV_PCK} not set - skipping full-corpus differential. See test docs for setup.",
            );
            return;
        }
    };
    let cache = match std::env::var(ENV_CACHE) {
        Ok(v) if !v.is_empty() => Some(PathBuf::from(v)),
        _ => {
            eprintln!(
                "{ENV_CACHE} not set - running crabby-only coverage assertion (no byte-compare).",
            );
            None
        }
    };

    let mut archive = PckArchive::open(&pck).expect("open RTV.pck");

    // Enumerate every .gdc under res://Scripts/ (or bare Scripts/).
    let targets: Vec<_> = archive
        .entries()
        .iter()
        .filter(|e| e.path.ends_with(".gdc"))
        .filter(|e| {
            let normalized = e.path.trim_start_matches("res://").trim_start_matches('/');
            normalized.starts_with("Scripts/")
        })
        .cloned()
        .collect();

    assert!(
        !targets.is_empty(),
        "no Scripts/*.gdc entries found in PCK - wrong file?",
    );

    eprintln!("running differential on {} scripts", targets.len());

    let mut reports = Vec::new();
    let mut detokenized_ok = 0usize;
    let mut matched_vostok = 0usize;
    let mut no_ground_truth: Vec<String> = Vec::new();

    for entry in &targets {
        let normalized = entry
            .path
            .trim_start_matches("res://")
            .trim_start_matches('/');
        let script_name = Path::new(normalized)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if script_name.is_empty() {
            continue;
        }

        let bytes = match archive.read(entry) {
            Ok(b) => b,
            Err(e) => {
                reports.push(Report {
                    entry_path: entry.path.clone(),
                    divergence: Divergence::CrabbyError {
                        message: format!("pck read failed: {e}"),
                    },
                });
                continue;
            }
        };

        // Coverage assertion #1: every script must detokenize without error.
        let actual = match detokenize(&bytes) {
            Ok(s) => s,
            Err(e) => {
                reports.push(Report {
                    entry_path: entry.path.clone(),
                    divergence: Divergence::CrabbyError {
                        message: format!("{e}"),
                    },
                });
                continue;
            }
        };
        detokenized_ok += 1;

        // Coverage assertion #2: if vostok has a cached entry, bytes match.
        let Some(cache_root) = cache.as_ref() else {
            continue;
        };
        let expected_path = cache_root.join(format!("Scripts/{script_name}.gd"));
        let Ok(expected) = fs::read_to_string(&expected_path) else {
            no_ground_truth.push(format!("{script_name} (size={})", entry.size));
            continue;
        };
        if actual == expected {
            matched_vostok += 1;
        } else {
            let first_diff = first_byte_diff(&actual, &expected);
            reports.push(Report {
                entry_path: entry.path.clone(),
                divergence: Divergence::ByteMismatch {
                    crabby_len: actual.len(),
                    vostok_len: expected.len(),
                    first_diff_byte: first_diff,
                    crabby_preview: window_around(&actual, first_diff),
                    vostok_preview: window_around(&expected, first_diff),
                },
            });
        }
    }

    eprintln!(
        "detokenized ok: {detokenized_ok} / {total}",
        total = targets.len(),
    );
    if cache.is_some() {
        eprintln!(
            "byte-matched vostok: {matched_vostok}   (no ground truth: {})",
            no_ground_truth.len(),
        );
    }

    if !reports.is_empty() {
        eprintln!("\n=== {} divergences ===", reports.len());
        for r in &reports {
            eprintln!("\n[{}]", r.entry_path);
            match &r.divergence {
                Divergence::CrabbyError { message } => {
                    eprintln!("  crabby error: {message}");
                }
                Divergence::ByteMismatch {
                    crabby_len,
                    vostok_len,
                    first_diff_byte,
                    crabby_preview,
                    vostok_preview,
                } => {
                    eprintln!("  byte mismatch at offset {first_diff_byte}");
                    eprintln!("  lengths: crabby={crabby_len} vostok={vostok_len}");
                    eprintln!("  crabby: {crabby_preview:?}");
                    eprintln!("  vostok: {vostok_preview:?}");
                }
            }
        }
        panic!(
            "{} script(s) failed coverage or byte-match out of {} total",
            reports.len(),
            targets.len(),
        );
    }
}

fn first_byte_diff(a: &str, b: &str) -> usize {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    for (i, (x, y)) in a_bytes.iter().zip(b_bytes.iter()).enumerate() {
        if x != y {
            return i;
        }
    }
    a_bytes.len().min(b_bytes.len())
}

/// Show ~40 bytes around a byte offset, with hex for non-printables.
fn window_around(s: &str, offset: usize) -> String {
    let bytes = s.as_bytes();
    let start = offset.saturating_sub(20);
    let end = (offset + 20).min(bytes.len());
    let slice = &bytes[start..end];
    String::from_utf8_lossy(slice)
        .replace('\n', "⏎")
        .replace('\t', "→")
}
