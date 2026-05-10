//! Small example: detokenize a `.gdc` file and print the reconstructed source.
//!
//! ```sh
//! cargo run --example detok -p crabby-detokenizer -- /path/to/Script.gdc
//! ```

use std::env;
use std::fs;
use std::path::PathBuf;

use crabby_detokenizer::detokenize;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path: PathBuf = env::args()
        .nth(1)
        .ok_or("usage: detok <path-to-gdc>")?
        .into();
    let bytes = fs::read(&path)?;
    let source = detokenize(&bytes)?;
    print!("{source}");
    Ok(())
}
