//! Cross-platform "open in browser" / "open in file manager" helpers.
//!
//! Wraps the [`opener`](https://crates.io/crates/opener) crate. Failures
//! are logged at `warn` rather than surfaced - opening an external
//! resource is a best-effort UX nicety, not a correctness path.

use std::path::Path;

/// Open a URL in the default browser.
pub fn open_url(url: &str) {
    if let Err(e) = opener::open_browser(url) {
        tracing::warn!(url, error = %e, "open: browser launch failed");
    }
}

/// Open a directory or file in the OS file manager.
///
/// For archive paths (`.vmz`, `.zip`, etc.) the file is selected in its
/// containing folder - most OSes do this with "reveal in explorer /
/// Finder / nautilus" semantics. The `opener` crate's `reveal` API does
/// exactly that on Win/macOS and falls back to opening the parent
/// directory on Linux.
pub fn reveal(path: &Path) {
    if let Err(e) = opener::reveal(path) {
        tracing::warn!(path = %path.display(), error = %e, "open: reveal failed");
    }
}

/// Open a directory in the OS file manager (no file selection).
/// Used for "Open mod folder" where the folder content should be visible
/// rather than the folder icon highlighted in its parent.
pub fn open_dir(path: &Path) {
    if let Err(e) = opener::open(path) {
        tracing::warn!(path = %path.display(), error = %e, "open: open_dir failed");
    }
}
