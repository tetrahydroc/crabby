//! Golden-fixture tests for [`rewrite_full_script`].
//!
//! Each fixture pair (`<stem>.input.gd` / `<stem>.expected.gd`) pins the
//! exact output shape of a full-script rewrite. Any template drift or
//! renamer regression surfaces as a byte diff here.

use std::fs;
use std::path::{Path, PathBuf};

use crabby_parser::parse_script;
use crabby_rewriter::rewrite_full_script;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[test]
fn multi_method_matches_expected() {
    // Mix of templates: one engine-void `_ready` → void; one `tick` with
    // declared return type → non-void; one static `util` → skipped.
    check_fixture("multi_method", "Example.gd");
}

#[test]
fn fast_template_matches_expected() {
    // Filename `MuzzleFlash.gd` puts this script in FAST_TEMPLATE_SCRIPTS,
    // so every hookable method gets the fast wrapper. The input's body
    // style intentionally mirrors what vostok-mod-loader's skip-list
    // targets look like.
    check_fixture("fast_template", "MuzzleFlash.gd");
}

#[test]
fn coroutine_ready_matches_expected() {
    // Coroutine handling is template-intrinsic, not a separate template:
    // when the parsed `FuncDecl.is_coroutine` is true, every vanilla-call
    // site in the wrapper gets an `await` prefix. The input's `_ready`
    // uses `await` in its body so the parser flags it; `update_fade`
    // does not, so its wrapper has no awaits. This fixture pins both
    // cases on one script to prevent drift.
    check_fixture("coroutine", "Message.gd");
}

#[test]
fn additive_template_matches_expected() {
    // Filename `WorldSave.gd` puts the script in ADDITIVE_TEMPLATE_SCRIPTS,
    // so vanilla method names stay put and wrappers are added alongside
    // under `_rtv_hooked_`. Covers both void (`save_data`) and non-void
    // (`has_key -> bool`) variants of the additive template in one pass.
    check_fixture("additive", "WorldSave.gd");
}

#[test]
fn data_intercept_matches_expected() {
    // Filename `ItemData.gd` puts the script in DATA_INTERCEPT_SCRIPTS,
    // so a `_get(property)` override + `_rtv_mod_patches` dict gets
    // appended at EOF. No method wrappers (the script has none).
    check_fixture("data_intercept", "ItemData.gd");
}

fn check_fixture(stem: &str, filename: &str) {
    let input_path = fixtures().join(format!("{stem}.input.gd"));
    let expected_path = fixtures().join(format!("{stem}.expected.gd"));

    let input = fs::read_to_string(&input_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", input_path.display()));
    let expected = fs::read_to_string(&expected_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", expected_path.display()));

    let parsed = parse_script(filename, &input).expect("parse");
    let actual = rewrite_full_script(&input, &parsed).expect("rewrite");

    if actual != expected {
        let dump = std::env::temp_dir().join(format!("crabby-{stem}-actual.gd"));
        let _ = fs::write(&dump, &actual);
        panic!(
            "rewritten output diverges from fixture.\n\
             actual:   {} ({} bytes)\n\
             expected: {} ({} bytes)",
            dump.display(),
            actual.len(),
            expected_path.display(),
            expected.len(),
        );
    }
}
