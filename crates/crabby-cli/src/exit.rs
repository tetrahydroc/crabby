//! Error-to-exit-code conversion.
//!
//! Binary code's single place where [`crabby_error::CrabbyError`] becomes a
//! process exit code + a user-readable error chain on stderr. Walks the
//! `#[source]` chain so the root cause isn't buried.

use std::error::Error;
use std::process::ExitCode;

use crabby_error::CrabbyError;

/// Print `err` and its `source` chain to stderr, return a failure exit code.
pub fn report(err: &CrabbyError) -> ExitCode {
    eprintln!("error: {err}");
    let mut cause: Option<&dyn Error> = err.source();
    while let Some(c) = cause {
        eprintln!("  caused by: {c}");
        cause = c.source();
    }
    ExitCode::FAILURE
}
