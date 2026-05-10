//! Reader for Godot PCK archives.
//!
//! Enumerates entries and reads their bytes. The detokenizer and rewriter
//! pipelines consume the output of this crate; no other crate should parse
//! PCK structure directly.
//!
//! Supports PCK format V2 (Godot 4.0-4.5) and V3 (Godot 4.6+). Both are
//! standalone-PCK only - embedded PCKs (packs appended to an executable)
//! are out of scope; Road to Vostok ships its PCK as a standalone file.
//!
//! # Example
//!
//! ```no_run
//! use std::path::Path;
//! use crabby_pck::PckArchive;
//!
//! let mut archive = PckArchive::open(Path::new("RTV.pck"))?;
//! for entry in archive.entries() {
//!     println!("{} ({} bytes)", entry.path, entry.size);
//! }
//! # Ok::<(), crabby_error::CrabbyError>(())
//! ```
//!
//! # Error convention
//!
//! Every public function returns [`crabby_error::Result`]. Leaf failures
//! convert into [`CrabbyError::Pck`] with a `context` string describing the
//! specific operation and the `source` preserving the underlying cause:
//!
//! ```
//! use crabby_error::{CrabbyError, Result};
//!
//! fn read_header(bytes: &[u8]) -> Result<u32> {
//!     if bytes.len() < 4 {
//!         return Err(CrabbyError::Pck {
//!             context: format!("header: need 4 bytes, got {}", bytes.len()),
//!             source: Box::new(std::io::Error::new(
//!                 std::io::ErrorKind::UnexpectedEof,
//!                 "short header",
//!             )),
//!         });
//!     }
//!     Ok(u32::from_le_bytes(bytes[..4].try_into().unwrap()))
//! }
//!
//! assert!(read_header(&[1, 2, 3]).is_err());
//! assert_eq!(read_header(&[1, 0, 0, 0]).unwrap(), 1);
//! ```
//!
//! [`CrabbyError::Pck`]: crabby_error::CrabbyError::Pck

#![deny(missing_docs)]

mod archive;
mod format;
mod writer;

pub use archive::{PckArchive, PckEntry};
pub use writer::PckWriter;
