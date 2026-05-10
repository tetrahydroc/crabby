//! Small example: extract named entries from a PCK to files on disk.
//!
//! Used to materialize differential-test fixtures for crabby-detokenizer.
//!
//! ```sh
//! cargo run --example extract -p crabby-pck -- \
//!     "/path/to/RTV.pck" ./out \
//!     Scripts/Hitbox.gdc Scripts/Camera.gdc
//! ```

#![allow(clippy::case_sensitive_file_extension_comparisons)]

use std::env;
use std::fs;
use std::path::PathBuf;

use crabby_pck::PckArchive;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let pck: PathBuf = args
        .next()
        .ok_or("usage: extract <pck> <outdir> <paths...>")?
        .into();
    let outdir: PathBuf = args.next().ok_or("missing outdir")?.into();
    let wants: Vec<String> = args.collect();
    if wants.is_empty() {
        return Err("no paths specified".into());
    }

    fs::create_dir_all(&outdir)?;
    let mut archive = PckArchive::open(&pck)?;

    for want in &wants {
        let entry = archive
            .entries()
            .iter()
            .find(|e| {
                e.path == *want
                    || e.path.ends_with(&format!("/{want}"))
                    || e.path.trim_start_matches("res://") == want
            })
            .cloned()
            .ok_or_else(|| format!("no entry matched {want:?}"))?;

        let bytes = archive.read(&entry)?;
        let out_file = outdir.join(want.rsplit('/').next().unwrap_or(want));
        fs::write(&out_file, &bytes)?;
        println!(
            "wrote {} ({} bytes) from {}",
            out_file.display(),
            bytes.len(),
            entry.path
        );
    }
    Ok(())
}
