//! Shared `tracing` subscriber setup.
//!
//! Binaries call [`init_tracing`] at the top of `main`. The function is
//! idempotent-per-process (a second call is a no-op; `tracing` refuses to
//! install two global subscribers and that is treated as success).
//!
//! # Format selection
//!
//! - `CRABBY_LOG_JSON=1` → JSON output (for CI log assertions, log aggregators).
//! - otherwise → compact human-readable output (for local development).
//!
//! # Level filter
//!
//! Honors `RUST_LOG` if set. Falls back to `info` otherwise.
//!
//! # Example
//!
//! ```no_run
//! use crabby_platform::observability::init_tracing;
//!
//! fn main() {
//!     init_tracing();
//!     tracing::info!("ready");
//! }
//! ```

use std::env;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;

/// Environment variable that switches the formatter from compact to JSON.
pub const JSON_ENV_VAR: &str = "CRABBY_LOG_JSON";

/// Install the process-wide `tracing` subscriber.
///
/// Safe to call more than once; a second invocation silently no-ops because
/// `tracing` rejects double-install and that shouldn't be fatal (binaries
/// may share code paths that each call `init_tracing`).
pub fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let use_json = env::var(JSON_ENV_VAR).is_ok_and(|v| v == "1");

    // `try_init` returns `Err` when a global subscriber is already installed;
    // that's the "called twice" case and is treated as success.
    let result = if use_json {
        fmt().with_env_filter(filter).json().try_init()
    } else {
        fmt().with_env_filter(filter).compact().try_init()
    };
    let _ = result;
}
