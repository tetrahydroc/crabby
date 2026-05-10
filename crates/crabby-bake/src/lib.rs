//! Bake orchestrator: composes the bake pipeline as a pure function from
//! inputs to artifacts.
//!
//! [`bake_pack`] rewrites every `Scripts/*.gdc` in the PCK using the
//! full-script rewriter (all five templates), runs the consumer call-site
//! rewriter, and emits a single hook pack archive.
//!
//! Filesystem placement of those bytes into a game directory is
//! `crabby-install`'s job.
//!
//! # Error convention
//!
//! Sub-crate errors reach the caller as-is. Orchestration-level failures
//! that don't map to a sub-crate convert into [`CrabbyError::Bake`]:
//!
//! ```
//! use crabby_error::{CrabbyError, Result};
//!
//! fn require_non_empty_modset(mods: &[&str]) -> Result<()> {
//!     if mods.is_empty() {
//!         return Err(CrabbyError::Bake {
//!             context: "no mods enabled; bake would produce a pass-through pack".into(),
//!             source: "at least one mod must be enabled".into(),
//!         });
//!     }
//!     Ok(())
//! }
//!
//! assert!(require_non_empty_modset(&["only_mod"]).is_ok());
//! assert!(require_non_empty_modset(&[]).is_err());
//! ```
//!
//! [`CrabbyError::Bake`]: crabby_error::CrabbyError::Bake

#![deny(missing_docs)]

mod bake;
mod bake_key;
mod bake_pck;

pub use bake::{BakeInputs, BakeOutputs, BakeStats, bake_pack};
pub use bake_key::{BakeKey, mods_digest_from_kinds};
pub use bake_pck::{BakePckInputs, BakePckOutputs, bake_pck};
