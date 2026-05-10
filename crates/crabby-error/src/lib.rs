//! Canonical error type for the crabby-loader workspace.
//!
//! Every crate in the workspace converts its local errors into [`CrabbyError`]
//! at its public boundary. This keeps the error surface across the bake
//! pipeline uniform and preserves `#[source]` chains from leaf to orchestrator.
//!
//! # Example
//!
//! ```
//! use crabby_error::{CrabbyError, Result};
//!
//! fn parse_something(input: &str) -> Result<u32> {
//!     input.parse().map_err(|e: std::num::ParseIntError| {
//!         CrabbyError::Manifest {
//!             source: Box::new(e),
//!             context: format!("parsing {input:?} as u32"),
//!         }
//!     })
//! }
//! ```

#![deny(missing_docs)]

use std::error::Error as StdError;
use std::path::PathBuf;

use thiserror::Error;

/// Result alias with the crate-wide error type baked in.
pub type Result<T> = std::result::Result<T, CrabbyError>;

/// Canonical error variant set for every public surface in the workspace.
///
/// Variants group failures by subsystem, not by leaf cause. The `source`
/// fields carry the underlying cause so call sites can walk the chain via
/// [`std::error::Error::source`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CrabbyError {
    /// I/O failure. Carries the path that was being operated on when known.
    #[error("i/o error at {path:?}: {source}")]
    Io {
        /// Path the operation was targeting, or `None` for pathless I/O.
        path: Option<PathBuf>,
        /// Underlying I/O cause.
        #[source]
        source: std::io::Error,
    },

    /// PCK parsing failed.
    #[error("pck: {context}")]
    Pck {
        /// Free-form explanatory context.
        context: String,
        /// Underlying cause.
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },

    /// GDSC detokenization failed.
    #[error("detokenize: {context}")]
    Detokenize {
        /// Free-form explanatory context.
        context: String,
        /// Underlying cause.
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },

    /// `GDScript` parsing failed.
    #[error("parse: {context}")]
    Parse {
        /// Free-form explanatory context.
        context: String,
        /// Underlying cause.
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },

    /// Source rewriting failed.
    #[error("rewrite: {context}")]
    Rewrite {
        /// Free-form explanatory context.
        context: String,
        /// Underlying cause.
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },

    /// Pack emission failed.
    #[error("pack: {context}")]
    Pack {
        /// Free-form explanatory context.
        context: String,
        /// Underlying cause.
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },

    /// Manifest parse or validation failed.
    #[error("manifest: {context}")]
    Manifest {
        /// Free-form explanatory context.
        context: String,
        /// Underlying cause.
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },

    /// `mod_config.toml` parse, write, or semantic-validation failed.
    #[error("config: {context}")]
    Config {
        /// Free-form explanatory context.
        context: String,
        /// Underlying cause.
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },

    /// Scanner rejected mod source as violating the crabby API contract.
    #[error("scanner rejected {mod_id}: {reason}")]
    ScannerRejected {
        /// Mod id as declared in `mod.txt`.
        mod_id: String,
        /// User-readable reason the mod was rejected.
        reason: String,
    },

    /// Bake orchestration failed.
    #[error("bake: {context}")]
    Bake {
        /// Free-form explanatory context.
        context: String,
        /// Underlying cause.
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },

    /// Platform-abstraction failure (process spawn, path resolution, etc).
    #[error("platform: {context}")]
    Platform {
        /// Free-form explanatory context.
        context: String,
        /// Underlying cause.
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },
}

impl CrabbyError {
    /// Construct an [`Io`](Self::Io) variant with a known path.
    #[must_use]
    pub fn io_at(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: Some(path.into()),
            source,
        }
    }

    /// Construct an [`Io`](Self::Io) variant with no path context.
    #[must_use]
    pub const fn io(source: std::io::Error) -> Self {
        Self::Io { path: None, source }
    }
}
