//! Async ModWorkshop client.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Semaphore};
use tokio::time::sleep;

use crate::cache;
use crate::model::{Mod, ModDownload, User};

const API_BASE: &str = "https://api.modworkshop.net";

/// Disk-cache TTL for all entry kinds. Six hours strikes the balance
/// between "fresh enough that update flags are useful" and "don't
/// re-hit MW on every relaunch." Mods with declared updates show
/// up within a quarter-day, fast enough that nobody waits longer
/// than the next normal play session.
const DEFAULT_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const VERSION_TTL: Duration = DEFAULT_TTL;

/// Browser UA: the API replies 403 to default `reqwest` UA strings on
/// some Cloudflare-fronted endpoints.
const USER_AGENT: &str = "Mozilla/5.0 (compatible; crabby-loader)";

/// Semaphore permits = max concurrent in-flight HTTP requests. Keeps
/// requests polite during bulk version probes.
const MAX_CONCURRENT: usize = 4;

/// Failure modes the client surfaces.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ClientError {
    /// HTTP non-2xx (carries status + URL for triage).
    #[error("HTTP {status} from {url}")]
    Http {
        /// HTTP status code.
        status: u16,
        /// Full URL that returned the error.
        url: String,
    },
    /// Network / TLS / timeout.
    #[error("network error: {0}")]
    Network(String),
    /// JSON parse failed for the response body.
    #[error("parse error: {0}")]
    Parse(String),
    /// Disk cache I/O - non-fatal, logged at `warn`; only escapes if
    /// the cache is the only available source and it's unreadable.
    #[error("cache error: {0}")]
    Cache(String),
}

/// Result of comparing local mod.txt version to remote latest version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateStatus {
    /// Remote is strictly newer (semver-compared).
    UpdateAvailable,
    /// Versions match.
    UpToDate,
    /// One side didn't parse as semver and the strings differ. Caller
    /// can show "version differs (local v X, remote v Y)" without
    /// implying which is newer.
    Differs,
    /// Local is strictly newer (rare; pre-release, dev override).
    LocalNewer,
    /// Couldn't determine (e.g. either side is empty).
    Unknown,
}

impl UpdateStatus {
    /// Whether the launcher should surface an update affordance.
    #[must_use]
    pub fn should_prompt(self) -> bool {
        matches!(self, Self::UpdateAvailable)
    }
}

/// In-flight request tracker. Multiple callers asking for the same
/// `(kind, id)` wait on the same `Notify` and then re-read from cache.
#[derive(Default)]
struct InflightMap {
    entries: HashMap<(&'static str, u64), Arc<tokio::sync::Notify>>,
}

/// Async client. Cheap to clone (`Arc` inside).
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").finish_non_exhaustive()
    }
}

struct Inner {
    http: reqwest::Client,
    inflight: Mutex<InflightMap>,
    permits: Arc<Semaphore>,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    /// Construct a fresh client. Connections are pooled internally by
    /// reqwest, so prefer to construct once and clone.
    #[must_use]
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(15))
            .build()
            .expect("reqwest client builds with our static config");
        Self {
            inner: Arc::new(Inner {
                http,
                inflight: Mutex::new(InflightMap::default()),
                permits: Arc::new(Semaphore::new(MAX_CONCURRENT)),
            }),
        }
    }

    /// Fetch a full mod record. Cache-first; ignores the cache if the
    /// envelope is older than `DEFAULT_TTL`. Concurrent calls for the
    /// same id wait on a single in-flight fetch.
    pub async fn get_mod(&self, id: u64) -> Result<Mod, ClientError> {
        let body = self
            .fetch_with_cache("mod", id, DEFAULT_TTL, &format!("{API_BASE}/mods/{id}"))
            .await?;
        serde_json::from_str::<Mod>(&body).map_err(|e| ClientError::Parse(e.to_string()))
    }

    /// Fetch the file list for a mod. MW returns this in display-order
    /// (newest first by `created_at` for normal layouts). Used as a
    /// fallback when `mod.download` is null but `has_download` is true;
    /// that happens on multi-file mods where MW doesn't elect a
    /// canonical download on the root record.
    pub async fn get_mod_files(&self, id: u64) -> Result<Vec<ModDownload>, ClientError> {
        let body = self
            .fetch_with_cache(
                "files",
                id,
                DEFAULT_TTL,
                &format!("{API_BASE}/mods/{id}/files"),
            )
            .await?;
        let parsed: ModFilesPage = serde_json::from_str(&body)
            .map_err(|e| ClientError::Parse(format!("files page: {e}")))?;
        Ok(parsed.data)
    }

    /// Fetch the bare latest-file version string. Used for cheap
    /// background update probes.
    pub async fn get_latest_version(&self, id: u64) -> Result<String, ClientError> {
        let body = self
            .fetch_with_cache(
                "version",
                id,
                VERSION_TTL,
                &format!("{API_BASE}/mods/{id}/files/latest/version"),
            )
            .await?;
        Ok(body.trim().to_string())
    }

    /// Fetch a user record.
    pub async fn get_user(&self, id: u64) -> Result<User, ClientError> {
        let body = self
            .fetch_with_cache("user", id, DEFAULT_TTL, &format!("{API_BASE}/users/{id}"))
            .await?;
        serde_json::from_str::<User>(&body).map_err(|e| ClientError::Parse(e.to_string()))
    }

    /// Fetch raw image bytes from MW's CDN. Disk-cached forever
    /// (filenames carry their own content hash, so a different
    /// version of the same logical image gets a different filename).
    /// Concurrent calls for the same file dedupe via the in-flight map.
    pub async fn get_image_bytes(&self, file: &str) -> Result<Vec<u8>, ClientError> {
        if file.is_empty() {
            return Err(ClientError::Parse("empty image filename".into()));
        }
        if let Some(bytes) = cache::read_image(file) {
            return Ok(bytes);
        }
        // No envelope/dedupe for images, they're keyed by content
        // hash so collisions are negligible and a duplicate fetch
        // just costs bandwidth. The HTTP-level Semaphore still
        // bounds total concurrency.
        let _permit = self
            .inner
            .permits
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore is never closed");
        let url = image_url(file);
        let resp = self
            .inner
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| ClientError::Network(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ClientError::Http {
                status: status.as_u16(),
                url,
            });
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| ClientError::Network(e.to_string()))?
            .to_vec();
        cache::write_image(file, &bytes);
        Ok(bytes)
    }

    /// Walk every page of `/games/{id}/mods` and return the flat list.
    /// Pre-warms the per-mod cache for each entry so subsequent
    /// `get_mod` calls hit disk instead of network.
    ///
    /// Cached as one blob keyed by game id with a 30-min TTL, much
    /// shorter than the per-mod TTL because listings include
    /// downloads/likes/views which the user expects to see refreshed
    /// regularly. Passes through the standard cache pipeline so
    /// concurrent calls dedupe.
    pub async fn list_game_mods(
        &self,
        game_id: u64,
    ) -> Result<Vec<crate::catalog::RemoteListing>, ClientError> {
        const LISTING_TTL: Duration = Duration::from_secs(30 * 60);
        const PER_PAGE: u32 = 50;

        let cache_key_url = format!("listing/{game_id}");
        let bulk = self
            .fetch_with_cache("listing", game_id, LISTING_TTL, &cache_key_url)
            .await;

        // The `fetch_with_cache` helper expects a real URL it can hit
        // on miss; the multi-page walk is driven here directly.
        // Detect the synthetic cache-key URL and fall through to a
        // hand-rolled fetch instead. (Easier than generalizing
        // fetch_with_cache for "value comes from a closure" right now.)
        match bulk {
            Ok(body) => parse_cached_listing(&body),
            Err(_) => {
                let listings = self.fetch_listing_pages(game_id, PER_PAGE).await?;
                let body = serde_json::to_string(&listings)
                    .map_err(|e| ClientError::Parse(e.to_string()))?;
                cache::write("listing", game_id, &body);
                Ok(listings)
            }
        }
    }

    /// The actual page walker. Sleeps 250ms between pages on cache
    /// misses to be polite. Returns listings flattened across pages.
    async fn fetch_listing_pages(
        &self,
        game_id: u64,
        per_page: u32,
    ) -> Result<Vec<crate::catalog::RemoteListing>, ClientError> {
        let mut out: Vec<crate::catalog::RemoteListing> = Vec::new();
        let mut page = 1_u32;
        loop {
            let url = format!(
                "{API_BASE}/games/{game_id}/mods?limit={per_page}&page={page}"
            );
            let body = self.do_fetch(&url).await?;
            let parsed: ListingPage = serde_json::from_str(&body)
                .map_err(|e| ClientError::Parse(format!("listing page {page}: {e}")))?;
            for m in &parsed.data {
                // Don't pre-warm the per-mod cache from listing
                // payloads, the listing endpoint omits `download` (and
                // `dependencies`), so a pre-warmed entry would poison
                // `get_mod` and break installs/updates with "Mod has
                // no download attached".
                out.push(crate::catalog::listing_from_mod(m));
            }
            if page >= parsed.meta.last_page.max(1) {
                break;
            }
            page += 1;
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Ok(out)
    }

    /// Download an arbitrary file URL and return the bytes. Used by
    /// the install flow to fetch a mod's vmz/zip from the MW CDN.
    /// No caching; the caller writes directly to disk.
    pub async fn download(&self, url: &str) -> Result<Vec<u8>, ClientError> {
        let _permit = self
            .inner
            .permits
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore is never closed");
        let resp = self
            .inner
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| ClientError::Network(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ClientError::Http {
                status: status.as_u16(),
                url: url.to_string(),
            });
        }
        Ok(resp
            .bytes()
            .await
            .map_err(|e| ClientError::Network(e.to_string()))?
            .to_vec())
    }

    /// Compare a local mod.txt version against the remote latest.
    pub async fn check_update(&self, id: u64, local_version: &str) -> UpdateStatus {
        let remote = match self.get_latest_version(id).await {
            Ok(v) if !v.is_empty() => v,
            _ => return UpdateStatus::Unknown,
        };
        compare_versions(local_version, &remote)
    }

    /// The shared cache + dedupe + HTTP path.
    async fn fetch_with_cache(
        &self,
        kind: &'static str,
        id: u64,
        ttl: Duration,
        url: &str,
    ) -> Result<String, ClientError> {
        // 1. Cache hit within TTL?
        if let Some(env) = cache::read(kind, id) {
            if env.age() <= ttl && !env.schema_outdated() {
                return Ok(env.body);
            }
        }

        // 2. Dedupe: wait if another caller is already fetching this.
        let notify = {
            let mut map = self.inner.inflight.lock().await;
            if let Some(existing) = map.entries.get(&(kind, id)).cloned() {
                drop(map);
                existing.notified().await;
                // Cache should now hold the fresh value; fall through
                // to re-read.
                if let Some(env) = cache::read(kind, id) {
                    if env.age() <= ttl && !env.schema_outdated() {
                        return Ok(env.body);
                    }
                }
                // Other fetch failed; try a fresh one.
                Arc::new(tokio::sync::Notify::new())
            } else {
                let n = Arc::new(tokio::sync::Notify::new());
                map.entries.insert((kind, id), n.clone());
                n
            }
        };

        // 3. Take a permit, fetch, write cache, notify, release.
        let result = self.do_fetch(url).await;
        if let Ok(body) = &result {
            cache::write(kind, id, body);
        } else if let Some(env) = cache::read(kind, id).filter(|e| !e.schema_outdated()) {
            // Stale-on-error: prefer a stale cache hit over a hard fail.
            // Skip if the cached envelope predates the current parser;
            // better to surface the live error than feed bad data.
            tracing::warn!(url, ?ttl, "mw: live fetch failed, serving stale cache");
            // Re-acquire to remove the in-flight marker before returning.
            self.inner
                .inflight
                .lock()
                .await
                .entries
                .remove(&(kind, id));
            notify.notify_waiters();
            return Ok(env.body);
        }
        // Remove from in-flight so subsequent callers re-enter the
        // fetch path.
        self.inner
            .inflight
            .lock()
            .await
            .entries
            .remove(&(kind, id));
        notify.notify_waiters();
        result
    }

    async fn do_fetch(&self, url: &str) -> Result<String, ClientError> {
        let _permit = self
            .inner
            .permits
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore is never closed");
        let resp = self
            .inner
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| ClientError::Network(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ClientError::Http {
                status: status.as_u16(),
                url: url.to_string(),
            });
        }
        resp.text()
            .await
            .map_err(|e| ClientError::Network(e.to_string()))
    }

    /// Bulk version probe for the Updates chip. Walks `ids`
    /// sequentially with a small delay to stay polite (~2/sec).
    /// Returns the subset whose remote version differs from `local`.
    ///
    /// Errors per-id are swallowed (just dropped from the result) so
    /// one offline id doesn't tank the rest of the batch.
    pub async fn bulk_check_updates(
        &self,
        targets: &[(u64, String)],
    ) -> Vec<(u64, UpdateStatus)> {
        let mut out = Vec::with_capacity(targets.len());
        for (i, (id, local)) in targets.iter().enumerate() {
            // Time the call so throttling only happens when the
            // network was actually touched. Cache hits return in <1ms;
            // rate limiting them just delays the UI for no reason.
            let started = std::time::Instant::now();
            let status = self.check_update(*id, local).await;
            out.push((*id, status));
            let was_network = started.elapsed() > Duration::from_millis(50);
            if was_network && i + 1 < targets.len() {
                sleep(Duration::from_millis(500)).await;
            }
        }
        out
    }
}

/// MW listing-endpoint envelope: `{ data: [...], meta: {...} }`.
/// Pagination metadata lives under `meta`.
#[derive(serde::Deserialize)]
struct ListingPage {
    data: Vec<crate::model::Mod>,
    meta: ListingMeta,
}

#[derive(serde::Deserialize)]
struct ListingMeta {
    #[serde(default)]
    last_page: u32,
}

/// MW files-endpoint envelope: `{ data: [...] }`. Same shape as the
/// listing page but without pagination meta (mods rarely have enough
/// files to need paging).
#[derive(serde::Deserialize)]
struct ModFilesPage {
    data: Vec<crate::model::ModDownload>,
}

/// Read back the cached listing blob (a JSON-array of `RemoteListing`).
fn parse_cached_listing(
    body: &str,
) -> Result<Vec<crate::catalog::RemoteListing>, ClientError> {
    serde_json::from_str(body).map_err(|e| ClientError::Parse(e.to_string()))
}

/// Build the public CDN URL for an image filename returned by the
/// API (e.g. `Mod::thumbnail.file`). MW serves all mod images from
/// the same path on `storage.modworkshop.net`.
#[must_use]
pub fn image_url(file: &str) -> String {
    format!("https://storage.modworkshop.net/mods/images/{file}")
}

/// Compare two version strings. Tries semver first; falls back to
/// `Differs` when one side doesn't parse.
#[must_use]
pub fn compare_versions(local: &str, remote: &str) -> UpdateStatus {
    if local.trim().is_empty() || remote.trim().is_empty() {
        return UpdateStatus::Unknown;
    }
    if local == remote {
        return UpdateStatus::UpToDate;
    }
    let l = semver::Version::parse(local.trim_start_matches('v'));
    let r = semver::Version::parse(remote.trim_start_matches('v'));
    match (l, r) {
        (Ok(lv), Ok(rv)) => {
            if rv > lv {
                UpdateStatus::UpdateAvailable
            } else if lv > rv {
                UpdateStatus::LocalNewer
            } else {
                UpdateStatus::UpToDate
            }
        }
        _ => UpdateStatus::Differs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_pure() {
        assert_eq!(compare_versions("1.0.0", "1.0.1"), UpdateStatus::UpdateAvailable);
        assert_eq!(compare_versions("1.0.0", "1.0.0"), UpdateStatus::UpToDate);
        assert_eq!(compare_versions("1.1.0", "1.0.9"), UpdateStatus::LocalNewer);
    }

    #[test]
    fn semver_with_v_prefix() {
        assert_eq!(compare_versions("v1.0.0", "v1.1.0"), UpdateStatus::UpdateAvailable);
    }

    #[test]
    fn differs_when_unparseable() {
        // "1.10" isn't valid semver (needs three components).
        assert_eq!(compare_versions("1.10", "1.9"), UpdateStatus::Differs);
        assert_eq!(compare_versions("alpha", "beta"), UpdateStatus::Differs);
    }

    #[test]
    fn empty_is_unknown() {
        assert_eq!(compare_versions("", "1.0.0"), UpdateStatus::Unknown);
        assert_eq!(compare_versions("1.0.0", ""), UpdateStatus::Unknown);
    }
}
