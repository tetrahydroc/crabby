//! Golden-fixture test for [`rewrite_single_method`].
//!
//! Pins the output shape of the void dispatch wrapper. If the wrapper
//! template changes deliberately, regenerate the `.expected.gd` file;
//! otherwise a diff here is a regression.

use std::fs;
use std::path::{Path, PathBuf};

use crabby_parser::parse_script;
use crabby_rewriter::rewrite_single_method;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[test]
fn controller_physics_process_matches_expected() {
    let input_path = fixtures().join("controller_physics_process.input.gd");
    let expected_path = fixtures().join("controller_physics_process.expected.gd");

    let input = fs::read_to_string(&input_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", input_path.display()));
    let expected = fs::read_to_string(&expected_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", expected_path.display()));

    let parsed = parse_script("Controller.gd", &input).expect("parse");
    let actual = rewrite_single_method(&input, &parsed, "_physics_process", "controller", "\t")
        .expect("rewrite");

    if actual != expected {
        let dump = std::env::temp_dir().join("crabby-controller-actual.gd");
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

#[test]
fn non_existent_method_errors() {
    let src = "extends Node\nfunc foo():\n\tpass\n";
    let parsed = parse_script("X.gd", src).expect("parse");
    let err = rewrite_single_method(src, &parsed, "missing", "x", "\t").expect_err("should fail");
    assert!(format!("{err}").contains("target method"), "got: {err}");
}

#[test]
fn non_void_method_rejected_in_p2_2() {
    let src = "\
extends Node

func returns_int() -> int:
\treturn 1
";
    let parsed = parse_script("X.gd", src).expect("parse");
    let err =
        rewrite_single_method(src, &parsed, "returns_int", "x", "\t").expect_err("should fail");
    assert!(format!("{err}").contains("non-void"), "got: {err}");
}
