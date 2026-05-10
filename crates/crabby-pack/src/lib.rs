//! ZIP pack emission, override.cfg generation, shim materialization.
//!
//! Takes rewritten source + shim source and produces the artifacts Godot
//! mounts at launch:
//!
//! - `framework_pack.zip` (rewritten scripts with the 3-entry recipe per file)
//! - `override.cfg` (`autoload_prepend` entries routed through the shim)
//! - Shim `.gd` files copied into the pack
//!
//! # The 3-entry recipe
//!
//! For each rewritten vanilla script, the pack contains three entries:
//!
//! 1. `Scripts/<Name>.gd` - the rewritten source bytes
//! 2. `Scripts/<Name>.gd.remap` - a self-referencing `[remap]` pointing at
//!    the `.gd` itself, defeating the vanilla PCK's `.gd → .gdc` redirect
//! 3. `Scripts/<Name>.gdc` - zero bytes, defeats Godot's sibling-bytecode
//!    preference so the loader falls back to compiling the `.gd`
//!
//! Ported from vostok-mod-loader's `_generate_hook_pack`.
//!
//! # Error convention
//!
//! Leaf failures convert into [`CrabbyError::Pack`]:
//!
//! ```
//! use crabby_error::{CrabbyError, Result};
//!
//! fn validate_zip_entry(path: &str) -> Result<()> {
//!     if path.contains('\\') {
//!         return Err(CrabbyError::Pack {
//!             context: format!("zip entry {path:?} uses backslash separator"),
//!             source: "Windows-style paths break Godot's pack mount".into(),
//!         });
//!     }
//!     Ok(())
//! }
//!
//! assert!(validate_zip_entry("res://ok/path.gd").is_ok());
//! assert!(validate_zip_entry("res:\\bad\\path.gd").is_err());
//! ```
//!
//! [`CrabbyError::Pack`]: crabby_error::CrabbyError::Pack

#![deny(missing_docs)]

mod canary;
mod entry;
mod pack;

pub use canary::{CANARY_ENTRY_NAME, CANARY_PREFIX, canary_content};
pub use entry::RewrittenScript;
pub use pack::{PackInputs, PackOutputs, emit_pack};
