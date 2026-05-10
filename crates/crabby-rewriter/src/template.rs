//! Wrapper-template dispatch.
//!
//! The rewriter picks one of the templates in this module per method or
//! per script (data-intercept) based on
//! [`pick_template`](crate::pick_template):
//!
//! - [`void`], void methods on ordinary scripts. Full dispatch with
//!   re-entry guard.
//! - [`non_void`], methods returning a value; void-template body shape
//!   with `_result` threading.
//! - [`fast`], lighter wrapper for per-frame / per-shot scripts
//!   (`MuzzleFlash`, `Hit`, `Mine`). Keeps dispatch but drops re-entry
//!   guard and `_caller` save/restore.
//! - [`additive`], no rename of the vanilla body; wrapper method sits
//!   alongside under a distinct prefix. Used for `Resource`-subclass
//!   scripts whose method names are persisted in save files.
//! - [`data_intercept`], script-level injection (no per-method
//!   wrapping). Adds a `_get(property)` override plus the registry's
//!   override dict so `lib.patch(...)` can shadow `@export var` values
//!   at runtime.

pub mod additive;
pub mod data_intercept;
pub mod fast;
pub mod non_void;
pub mod void;
