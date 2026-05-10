//! Thin cross-platform abstractions for the crabby-loader workspace.
//!
//! Domain crates call into this crate instead of using `#[cfg(windows)]` or
//! `#[cfg(unix)]` directly. Platform differences that leak into domain logic
//! are bugs.
//!
//! Also hosts the shared [`observability`] module so binaries (`crabby-cli`,
//! `crabby-ui`) initialize `tracing` the same way.

#![deny(missing_docs)]

pub mod observability;
