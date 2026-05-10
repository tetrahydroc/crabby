//! On-disk cache for ModWorkshop responses.
//!
//! One file per (kind, id) under
//! `<user-config>/crabby/cache/modworkshop/`. Files are JSON with a
//! tiny envelope wrapping the original response so a fetch time can be
//! stamped without modifying the upstream payload.

use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

/// Resolve the cache root. `None` if the platform has no user-config dir.
#[must_use]
pub fn cache_dir() -> Option<PathBuf> {
    ProjectDirs::from("", "", "crabby")
        .map(|p| p.config_dir().join("cache").join("modworkshop"))
}

/// Path for a specific (kind, id) entry. `kind` is a short discriminator
/// like `"mod"`, `"version"`, `"user"`.
#[must_use]
pub fn cache_path(kind: &str, id: u64) -> Option<PathBuf> {
    cache_dir().map(|d| d.join(format!("{kind}-{id}.json")))
}

/// Path for a binary image blob, keyed by storage filename. We keep
/// these in a sibling `images/` dir so they don't pollute the JSON
/// envelopes and are easy to nuke as a group.
#[must_use]
pub fn image_cache_path(file: &str) -> Option<PathBuf> {
    cache_dir().map(|d| d.join("images").join(sanitize_filename(file)))
}

/// Strip path-traversal characters from a filename so a maliciously
/// crafted MW response can't write outside the cache dir. Real MW
/// filenames are flat (no slashes, no `..`) but defended anyway.
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| if matches!(c, '/' | '\\' | '\0') { '_' } else { c })
        .collect()
}

/// Read raw image bytes from disk. None if missing/unreadable.
pub(crate) fn read_image(file: &str) -> Option<Vec<u8>> {
    let path = image_cache_path(file)?;
    fs::read(&path).ok()
}

/// Write image bytes. Best-effort; failures are logged at warn.
pub(crate) fn write_image(file: &str, bytes: &[u8]) {
    let Some(path) = image_cache_path(file) else {
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            tracing::warn!(path = %parent.display(), error = %e, "mw image cache: mkdir failed");
            return;
        }
    }
    if let Err(e) = fs::write(&path, bytes) {
        tracing::warn!(path = %path.display(), error = %e, "mw image cache: write failed");
    }
}

/// Bump when a parser fix means existing cached payloads should be
/// treated as stale even if they're inside their TTL. Cached files
/// without this marker, or with a smaller value, get refetched.
const CACHE_SCHEMA: u32 = 3;

/// Cache envelope. `fetched_at` is Unix seconds; `body` is the raw
/// response payload (string, not parsed JSON, so fields are preserved
/// on schema drift).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Envelope {
    pub fetched_at: u64,
    pub body: String,
    /// Cache-format version. Missing in old envelopes; defaults to 0
    /// so they're invalidated by the next [`CACHE_SCHEMA`] bump.
    #[serde(default)]
    pub schema: u32,
}

impl Envelope {
    pub fn fresh(body: String) -> Self {
        let fetched_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self { fetched_at, body, schema: CACHE_SCHEMA }
    }

    pub fn age(&self) -> Duration {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Duration::from_secs(now.saturating_sub(self.fetched_at))
    }

    /// True when the envelope was written by an outdated parser and
    /// should be refetched regardless of age.
    pub fn schema_outdated(&self) -> bool {
        self.schema < CACHE_SCHEMA
    }
}

/// Read an envelope from disk. Returns `None` for missing/unreadable
/// files; caller decides whether that means "fetch fresh" or "give up".
pub(crate) fn read(kind: &str, id: u64) -> Option<Envelope> {
    let path = cache_path(kind, id)?;
    let mut f = fs::File::open(&path).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    serde_json::from_str(&s).ok()
}

/// Write an envelope. Best-effort; failures are logged at `warn` and
/// swallowed (cache is an optimization, not a correctness requirement).
pub(crate) fn write(kind: &str, id: u64, body: &str) {
    let Some(path) = cache_path(kind, id) else {
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            tracing::warn!(path = %parent.display(), error = %e, "mw cache: mkdir failed");
            return;
        }
    }
    let env = Envelope::fresh(body.to_string());
    let json = match serde_json::to_string(&env) {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!(error = %e, "mw cache: serialize failed");
            return;
        }
    };
    match fs::File::create(&path) {
        Ok(mut f) => {
            if let Err(e) = f.write_all(json.as_bytes()) {
                tracing::warn!(path = %path.display(), error = %e, "mw cache: write failed");
            }
        }
        Err(e) => tracing::warn!(path = %path.display(), error = %e, "mw cache: create failed"),
    }
}
