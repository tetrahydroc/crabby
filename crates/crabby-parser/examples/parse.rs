//! Small example: detokenize + parse a `.gdc`, dump the structure.
//!
//! ```sh
//! cargo run --example parse -p crabby-parser -- \
//!     /path/to/Controller.gdc
//! ```

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crabby_detokenizer::detokenize;
use crabby_parser::parse_script;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path: PathBuf = env::args()
        .nth(1)
        .ok_or("usage: parse <path-to-gdc>")?
        .into();
    let bytes = fs::read(&path)?;
    let source = detokenize(&bytes)?;
    let filename = Path::new(&path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map_or_else(|| "Unknown.gd".into(), |s| format!("{s}.gd"));

    let parsed = parse_script(&filename, &source)?;

    println!("filename:    {}", parsed.filename);
    println!("path:        {}", parsed.path);
    println!("extends:     {:?}", parsed.extends);
    println!("class_name:  {:?}", parsed.class_name);
    println!(
        "top vars:    {} ({:?})",
        parsed.var_names.len(),
        parsed.var_names
    );
    println!("functions:   {}", parsed.functions.len());
    for f in &parsed.functions {
        let modifiers = if f.is_static { "static " } else { "" };
        let ret = f.return_type.as_deref().map_or("-", |s| s);
        let flags = [
            ("coroutine", f.is_coroutine),
            ("returns_value", f.has_return_value),
        ]
        .iter()
        .filter(|(_, v)| *v)
        .map(|(k, _)| *k)
        .collect::<Vec<_>>()
        .join(",");
        println!(
            "  L{:>4}  {}{}({}) -> {}   [{}]",
            f.line_number, modifiers, f.name, f.params, ret, flags,
        );
    }
    Ok(())
}
