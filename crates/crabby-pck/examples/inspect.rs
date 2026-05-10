//! Small example: open a PCK and print a summary.
//!
//! Run with:
//! ```sh
//! cargo run --example inspect -p crabby-pck -- "/path/to/RTV.pck"
//! ```

// PCK paths are deliberately case-sensitive (Godot resource paths), so the
// case-insensitive variant the lint prefers would be incorrect for our domain.
#![allow(clippy::case_sensitive_file_extension_comparisons)]

use std::env;
use std::path::PathBuf;

use crabby_pck::PckArchive;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path: PathBuf = env::args()
        .nth(1)
        .ok_or("usage: inspect <path-to-pck>")?
        .into();

    let archive = PckArchive::open(&path)?;
    let (gmaj, gmin, gpatch) = archive.godot_version();

    println!("file:            {}", archive.path().display());
    println!("format version:  v{}", archive.format_version());
    println!("godot version:   {gmaj}.{gmin}.{gpatch}");
    println!("entry count:     {}", archive.entries().len());

    // Sample a handful of paths so we can eyeball the directory.
    let sample: Vec<_> = archive.entries().iter().take(5).map(|e| &e.path).collect();
    println!("first 5 entries: {sample:#?}");

    let scripts = archive
        .entries()
        .iter()
        .filter(|e| e.path.ends_with(".gd") || e.path.ends_with(".gdc"))
        .count();
    println!("*.gd / *.gdc:    {scripts}");

    // Pull the first .gdc found and dump its first 16 bytes; a real
    // pack should have the GDSC magic.
    let first_script = archive
        .entries()
        .iter()
        .find(|e| e.path.ends_with(".gdc"))
        .cloned();
    let Some(entry) = first_script else {
        return Ok(());
    };
    println!(
        "\nreading first .gdc: {} ({} bytes)",
        entry.path, entry.size
    );
    let mut archive = archive;
    let bytes = archive.read(&entry)?;
    let prefix: Vec<String> = bytes.iter().take(16).map(|b| format!("{b:02x}")).collect();
    println!("first 16 bytes:   {}", prefix.join(" "));
    let magic: String = bytes.iter().take(4).map(|&b| b as char).collect();
    println!("magic as ascii:   {magic:?}");
    Ok(())
}
