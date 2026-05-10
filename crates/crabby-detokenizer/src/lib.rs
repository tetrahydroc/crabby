//! GDSC v100/v101 binary-tokenized script → `GDScript` source.
//!
//! Godot ships binary-tokenized `.gdc` files inside the PCK; `load(...).source_code`
//! is empty for these at runtime. This crate reconstructs the textual source
//! so the rewriter pipeline has something to parse.
//!
//! Supports tokenizer v100 (Godot 4.0-4.4) and v101 (Godot 4.5-4.6).
//!
//! # Output conventions
//!
//! Output uses **tab indentation**, matching vostok-mod-loader's detokenizer
//! and Godot's canonical style. The PCK doesn't preserve the author's
//! original whitespace choice (only column numbers), so tabs-vs-spaces is a
//! forced pick.
//!
//! # Example
//!
//! ```no_run
//! use crabby_detokenizer::detokenize;
//!
//! let bytes = std::fs::read("Scripts/Hitbox.gdc").expect("read");
//! let source = detokenize(&bytes)?;
//! println!("{source}");
//! # Ok::<(), crabby_error::CrabbyError>(())
//! ```
//!
//! # Error convention
//!
//! Leaf failures convert into [`CrabbyError::Detokenize`]:
//!
//! ```
//! use crabby_error::{CrabbyError, Result};
//!
//! fn check_magic(bytes: &[u8]) -> Result<()> {
//!     if bytes.len() < 4 || &bytes[..4] != b"GDSC" {
//!         return Err(CrabbyError::Detokenize {
//!             context: "missing GDSC magic".into(),
//!             source: "not a tokenized gdc file".into(),
//!         });
//!     }
//!     Ok(())
//! }
//!
//! assert!(check_magic(b"").is_err());
//! assert!(check_magic(b"GDSC\x65\x00\x00\x00").is_ok());
//! ```
//!
//! [`CrabbyError::Detokenize`]: crabby_error::CrabbyError::Detokenize

#![deny(missing_docs)]

mod decode;
mod format;
mod frame;
mod reconstruct;
mod tokens;

pub use format::{SUPPORTED_VERSIONS, TokenizerVersion};
pub use frame::probe_version;

use crabby_error::Result;

/// Detokenize a `.gdc` byte payload into `GDScript` source.
///
/// Returns the reconstructed source text, which ends with `\n`. Tab-indented
/// per [the crate docs](self). Empty input → empty output (matches vostok's
/// handling of zero-byte PCK entries like `CasettePlayer.gd`).
pub fn detokenize(bytes: &[u8]) -> Result<String> {
    if bytes.is_empty() {
        return Ok(String::new());
    }
    let frame = frame::Frame::parse(bytes)?;
    let parsed = decode::parse(&frame)?;
    Ok(reconstruct::emit(&parsed))
}
