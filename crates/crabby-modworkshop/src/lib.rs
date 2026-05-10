//! ModWorkshop API client.
//!
//! Wraps the public read-only endpoints at `https://api.modworkshop.net/`.
//! No auth needed for GETs; the server rate-limits per-IP and returns
//! 429 if abused; request rate is kept sane and caching is aggressive.
//!
//! # What we use
//!
//! - `GET /mods/{id}` - full metadata (name, desc, version, downloads,
//!   likes, views, bumped_at, repo_url, download.version, etc.).
//! - `GET /mods/{id}/files/latest/version` - bare version string for
//!   cheap update probes.
//! - `GET /users/{id}` - author display name + avatar.
//!
//! # Caching
//!
//! Two layers:
//!
//! 1. **In-memory dedupe** - concurrent requests for the same id wait
//!    on a single in-flight fetch via a `tokio::sync::Mutex`-guarded
//!    map of `OnceCell`-style oneshots. Prevents fan-out when the UI
//!    re-renders and re-asks for the same data.
//! 2. **Disk cache** - JSON written under
//!    `<user-config>/crabby/cache/modworkshop/<id>.json`. Default TTL
//!    6 hours. Stale-on-error: if a fetch fails and a stale file
//!    exists, it is surfaced with a flag rather than nothing.
//!
//! # Hand-rolled client
//!
//! The OpenAPI spec leaves response schemas empty. The live endpoints return
//! consistent JSON, so a hand-typed `serde` model with `#[serde(default)]`
//! on every field is more reliable.

#![deny(missing_docs)]

mod cache;
pub mod catalog;
mod client;
mod model;

pub use cache::{cache_dir, cache_path, image_cache_path};
pub use catalog::{GameFilter, MwCatalog, RemoteCatalog, RemoteListing};
pub use client::{image_url, Client, ClientError, UpdateStatus};
pub use model::{Dependency, Image, Mod, ModDownload, Tag, User};
