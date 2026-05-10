//! Differential tests: detokenize a real vanilla `.gdc` fixture and assert
//! byte-equivalence against vostok-mod-loader's output for the same script.
//!
//! Fixtures live under `tests/fixtures/`:
//! - `gdc/<Name>.gdc` - raw bytes extracted from `RTV.pck`
//! - `expected/<Name>.gd` - vostok's detokenized output (ground truth)

use std::fs;
use std::path::Path;

use crabby_detokenizer::detokenize;

const fn manifest_dir() -> &'static str {
    env!("CARGO_MANIFEST_DIR")
}

fn check(name: &str) {
    let gdc = Path::new(manifest_dir())
        .join("tests/fixtures/gdc")
        .join(format!("{name}.gdc"));
    let expected_path = Path::new(manifest_dir())
        .join("tests/fixtures/expected")
        .join(format!("{name}.gd"));

    let bytes = fs::read(&gdc).unwrap_or_else(|e| panic!("read {}: {e}", gdc.display()));
    // Normalize CRLF to LF: .gitattributes pins fixtures to LF on
    // checkout, but a misconfigured contributor box (or a failed
    // attribute apply) shouldn't make the test diverge purely on
    // line endings. Detokenizer always outputs LF.
    let expected = fs::read_to_string(&expected_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", expected_path.display()))
        .replace("\r\n", "\n");

    let actual = detokenize(&bytes).expect("detokenize");

    if actual != expected {
        // Write actual to /tmp so `diff` can inspect on CI failure.
        let dump = std::env::temp_dir().join(format!("crabby-diff-{name}.gd"));
        let _ = fs::write(&dump, &actual);
        panic!(
            "detokenized output for {name} diverges from vostok ground truth.\n\
             actual:   {} ({} bytes)\n\
             expected: {} ({} bytes)",
            dump.display(),
            actual.len(),
            expected_path.display(),
            expected.len(),
        );
    }
}

#[test]
fn hitbox_matches_vostok() {
    check("Hitbox");
}

#[test]
fn camera_matches_vostok() {
    check("Camera");
}

#[test]
fn door_matches_vostok() {
    check("Door");
}

#[test]
fn pickup_matches_vostok() {
    check("Pickup");
}

#[test]
fn audio_matches_vostok() {
    check("Audio");
}
