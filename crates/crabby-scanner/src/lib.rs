//! Token-level security scanner.
//!
//! Hard-blocks mods that call forbidden APIs: `OS.execute`, `take_over_path`,
//! `set_script`, raw `FileAccess` outside the mod's sandbox, `ResourceLoader.load`
//! with non-literal arguments, and related patterns. Operates on the same
//! token stream the rewriter consumes; there is no regex-bypass surface.
//!
//! # Error convention
//!
//! Rejections convert into [`CrabbyError::ScannerRejected`], which names the
//! mod and surfaces a user-readable reason:
//!
//! ```
//! use crabby_error::{CrabbyError, Result};
//!
//! fn check_for_os_execute(mod_id: &str, source: &str) -> Result<()> {
//!     if source.contains("OS.execute") {
//!         return Err(CrabbyError::ScannerRejected {
//!             mod_id: mod_id.into(),
//!             reason: "calls to `OS.execute` are forbidden".into(),
//!         });
//!     }
//!     Ok(())
//! }
//!
//! assert!(check_for_os_execute("good_mod", "var x = 1\n").is_ok());
//! assert!(check_for_os_execute("bad_mod", "OS.execute(\"curl\", [])").is_err());
//! ```
//!
//! [`CrabbyError::ScannerRejected`]: crabby_error::CrabbyError::ScannerRejected

#![deny(missing_docs)]
