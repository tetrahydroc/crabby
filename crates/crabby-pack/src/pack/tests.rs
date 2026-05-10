//! Unit tests for [`emit_pack`].
//!
//! Each test emits a pack into a temp dir, opens the resulting ZIP with the
//! same crate's reader, and asserts per-entry content. Temp dirs clean up
//! on drop.

use std::fs;
use std::io::Read;
use std::path::PathBuf;

use zip::ZipArchive;

use super::*;
use crate::canary::{CANARY_ENTRY_NAME, CANARY_PREFIX};
use crate::entry::RewrittenScript;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "crabby-pack-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0),
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn read_entry(zip_path: &std::path::Path, name: &str) -> Option<Vec<u8>> {
    let file = fs::File::open(zip_path).expect("open zip");
    let mut archive = ZipArchive::new(file).expect("parse zip");
    let mut out = None;
    for i in 0..archive.len() {
        let mut e = archive.by_index(i).expect("read entry");
        if e.name() == name {
            let mut buf = Vec::new();
            e.read_to_end(&mut buf).expect("read bytes");
            out = Some(buf);
            break;
        }
    }
    out
}

fn all_entry_names(zip_path: &std::path::Path) -> Vec<String> {
    let file = fs::File::open(zip_path).expect("open zip");
    let mut archive = ZipArchive::new(file).expect("parse zip");
    (0..archive.len())
        .map(|i| archive.by_index(i).expect("read entry").name().to_owned())
        .collect()
}

fn sample_script() -> RewrittenScript {
    RewrittenScript {
        zip_path: "Scripts/Hitbox.gd".into(),
        rewritten_source: "extends Node3D\nclass_name Hitbox\n".into(),
        emit_empty_gdc: true,
    }
}

fn sample_additive_script() -> RewrittenScript {
    RewrittenScript {
        zip_path: "Scripts/SlotData.gd".into(),
        rewritten_source: "extends Resource\nclass_name SlotData\n".into(),
        emit_empty_gdc: false,
    }
}

#[test]
fn emits_three_entry_recipe_per_script_plus_canary() {
    let tmp = TempDir::new("3-entry");
    let out = tmp.path.join("framework_pack.zip");

    let outputs = emit_pack(&PackInputs {
        rewritten_scripts: &[sample_script()],
        out_path: &out,
        version: "0.1.0",
    })
    .expect("emit_pack");

    assert_eq!(outputs.script_entry_count, 1);
    assert_eq!(outputs.canary_content, "CRABBY-VFS-CANARY-0.1.0");

    let names = all_entry_names(&out);
    let expected = vec![
        CANARY_ENTRY_NAME.to_owned(),
        "Scripts/Hitbox.gd".to_owned(),
        "Scripts/Hitbox.gd.remap".to_owned(),
        "Scripts/Hitbox.gdc".to_owned(),
    ];
    assert_eq!(names, expected);
}

#[test]
fn gd_entry_contains_rewritten_source_verbatim() {
    let tmp = TempDir::new("gd-source");
    let out = tmp.path.join("pack.zip");
    let script = sample_script();
    let expected_source = script.rewritten_source.clone();

    emit_pack(&PackInputs {
        rewritten_scripts: &[script],
        out_path: &out,
        version: "0.1.0",
    })
    .expect("emit");

    let got = read_entry(&out, "Scripts/Hitbox.gd").expect("missing .gd entry");
    assert_eq!(got, expected_source.as_bytes());
}

#[test]
fn remap_points_at_the_original_res_path() {
    let tmp = TempDir::new("remap");
    let out = tmp.path.join("pack.zip");

    emit_pack(&PackInputs {
        rewritten_scripts: &[sample_script()],
        out_path: &out,
        version: "0.1.0",
    })
    .expect("emit");

    let got = read_entry(&out, "Scripts/Hitbox.gd.remap").expect("missing .remap");
    let got_str = String::from_utf8(got).expect("utf8");
    assert_eq!(got_str, "[remap]\npath=\"res://Scripts/Hitbox.gd\"\n");
}

#[test]
fn gdc_entry_is_empty() {
    let tmp = TempDir::new("empty-gdc");
    let out = tmp.path.join("pack.zip");

    emit_pack(&PackInputs {
        rewritten_scripts: &[sample_script()],
        out_path: &out,
        version: "0.1.0",
    })
    .expect("emit");

    let got = read_entry(&out, "Scripts/Hitbox.gdc").expect("missing .gdc");
    assert!(
        got.is_empty(),
        ".gdc should be zero bytes, got {}",
        got.len()
    );
}

#[test]
fn canary_content_embeds_version() {
    let tmp = TempDir::new("canary");
    let out = tmp.path.join("pack.zip");

    emit_pack(&PackInputs {
        rewritten_scripts: &[sample_script()],
        out_path: &out,
        version: "1.2.3",
    })
    .expect("emit");

    let got = read_entry(&out, CANARY_ENTRY_NAME).expect("missing canary");
    let got_str = String::from_utf8(got).expect("utf8");
    assert!(got_str.starts_with(CANARY_PREFIX), "{got_str}");
    assert!(got_str.ends_with("1.2.3"), "{got_str}");
}

#[test]
fn emit_overwrites_existing_pack() {
    let tmp = TempDir::new("overwrite");
    let out = tmp.path.join("pack.zip");

    // First emit: version 0.1.0.
    emit_pack(&PackInputs {
        rewritten_scripts: &[sample_script()],
        out_path: &out,
        version: "0.1.0",
    })
    .expect("first emit");
    let first_canary = read_entry(&out, CANARY_ENTRY_NAME).expect("canary after first emit");
    assert!(
        String::from_utf8_lossy(&first_canary).ends_with("0.1.0"),
        "first canary should carry 0.1.0",
    );

    // Second emit: different version, same out path - must overwrite.
    emit_pack(&PackInputs {
        rewritten_scripts: &[sample_script()],
        out_path: &out,
        version: "0.2.0",
    })
    .expect("second emit");
    let second_canary = read_entry(&out, CANARY_ENTRY_NAME).expect("canary after second emit");
    assert!(
        String::from_utf8_lossy(&second_canary).ends_with("0.2.0"),
        "second canary should carry 0.2.0",
    );
}

#[test]
fn creates_missing_parent_dir() {
    let tmp = TempDir::new("nested");
    let nested = tmp.path.join("deeply").join("nested");
    let out = nested.join("pack.zip");
    assert!(!nested.exists());

    emit_pack(&PackInputs {
        rewritten_scripts: &[sample_script()],
        out_path: &out,
        version: "0.1.0",
    })
    .expect("emit");

    assert!(out.is_file());
}

#[test]
fn rejects_backslash_in_zip_path() {
    let tmp = TempDir::new("bad-path");
    let out = tmp.path.join("pack.zip");

    let bad = RewrittenScript {
        zip_path: r"Scripts\Hitbox.gd".into(),
        rewritten_source: String::new(),
        emit_empty_gdc: true,
    };
    let err = emit_pack(&PackInputs {
        rewritten_scripts: &[bad],
        out_path: &out,
        version: "0.1.0",
    })
    .expect_err("should reject");
    assert!(format!("{err}").contains("backslash"), "got: {err}");
    assert!(
        !out.exists(),
        "no pack should be written on validation failure"
    );
}

#[test]
fn rejects_empty_version() {
    let tmp = TempDir::new("empty-version");
    let out = tmp.path.join("pack.zip");
    let err = emit_pack(&PackInputs {
        rewritten_scripts: &[sample_script()],
        out_path: &out,
        version: "",
    })
    .expect_err("should reject");
    assert!(format!("{err}").contains("version"), "got: {err}");
}

#[test]
fn additive_script_omits_empty_gdc_companion() {
    // Resource-typed (additive) scripts must NOT ship the empty `.gdc`,
    // it breaks Godot's resource-script class binding for `.tres`
    // references pointing at the script's path. Two entries only:
    // `.gd` + `.gd.remap`.
    let tmp = TempDir::new("additive-2-entry");
    let out = tmp.path.join("pack.zip");

    emit_pack(&PackInputs {
        rewritten_scripts: &[sample_additive_script()],
        out_path: &out,
        version: "0.1.0",
    })
    .expect("emit");

    let names = all_entry_names(&out);
    let expected = vec![
        CANARY_ENTRY_NAME.to_owned(),
        "Scripts/SlotData.gd".to_owned(),
        "Scripts/SlotData.gd.remap".to_owned(),
    ];
    assert_eq!(names, expected);
    assert!(
        read_entry(&out, "Scripts/SlotData.gdc").is_none(),
        "additive scripts must not emit an empty .gdc companion",
    );
}

#[test]
fn mixed_pack_emits_per_type_layout() {
    // Same archive: a Node script gets the full 3-entry recipe, an
    // additive Resource script gets only `.gd` + `.gd.remap`.
    let tmp = TempDir::new("mixed");
    let out = tmp.path.join("pack.zip");

    emit_pack(&PackInputs {
        rewritten_scripts: &[sample_script(), sample_additive_script()],
        out_path: &out,
        version: "0.1.0",
    })
    .expect("emit");

    let names = all_entry_names(&out);
    let expected = vec![
        CANARY_ENTRY_NAME.to_owned(),
        "Scripts/Hitbox.gd".to_owned(),
        "Scripts/Hitbox.gd.remap".to_owned(),
        "Scripts/Hitbox.gdc".to_owned(),
        "Scripts/SlotData.gd".to_owned(),
        "Scripts/SlotData.gd.remap".to_owned(),
    ];
    assert_eq!(names, expected);
}

#[test]
fn pack_with_zero_scripts_still_emits_canary() {
    let tmp = TempDir::new("no-scripts");
    let out = tmp.path.join("pack.zip");

    let outputs = emit_pack(&PackInputs {
        rewritten_scripts: &[],
        out_path: &out,
        version: "0.1.0",
    })
    .expect("emit");

    assert_eq!(outputs.script_entry_count, 0);
    assert!(read_entry(&out, CANARY_ENTRY_NAME).is_some());
    assert_eq!(all_entry_names(&out).len(), 1);
}
