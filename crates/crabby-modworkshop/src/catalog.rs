//! Source-pluggable remote-catalog interface.
//!
//! Today there's one implementation: [`MwCatalog`] wrapping the
//! ModWorkshop client. The [`RemoteCatalog`] trait is the seam where
//! a future Nexus / GitHub-releases / Steam Workshop backend would
//! plug in; the launcher's Browse view consumes the trait, not the
//! concrete impl.
//!
//! # Where listings live
//!
//! Each implementation owns its own cache. Caches are not merged
//! across sources (e.g. a mod that exists on both MW and Nexus is
//! still two `RemoteListing`s, deduped at display time by whatever
//! the launcher decides, typically by MW id when both sources
//! expose one).

use std::sync::Arc;

use crate::client::{Client, ClientError};
use crate::model::{Image, Mod, Tag, User};

/// One mod surfaced by a remote catalog. Intentionally narrow; the
/// launcher's row view + lightweight detail rendering only need this
/// much. Heavy data (full description, dependencies, download URL)
/// lands via [`RemoteCatalog::fetch_full`] when the user opens a mod.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RemoteListing {
    /// Catalog-issued id. Stable within a catalog; not unique across
    /// catalogs. Use `(source_id, listing.id)` for cross-catalog
    /// identity.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Catalog id of the source ("modworkshop", "nexus", "github", ...).
    /// Carried as `String` (not `&'static`) so listings round-trip
    /// through the on-disk cache cleanly.
    pub source_id: String,
    /// Author display name when supplied; empty otherwise.
    pub author: String,
    /// Author display name's avatar filename (catalog-relative); the
    /// caller resolves to a URL via the catalog's image_url helper.
    pub author_avatar: String,
    /// Mod version string as the author published it. May be empty.
    pub version: String,
    /// Lifetime download count, when the catalog reports it.
    pub downloads: u64,
    /// Like / favorite count.
    pub likes: u64,
    /// View count.
    pub views: u64,
    /// Thumbnail image record, when present.
    pub thumbnail: Option<Image>,
    /// Tag chips for filtering / display.
    pub tags: Vec<Tag>,
    /// Most-recent update timestamp from the source. Drives "is the
    /// cached mod-detail stale?" checks; when this advances, the
    /// per-mod cache is invalidated without needing to compare
    /// version strings.
    pub updated_at: String,
    /// Brief tagline shown on cards. May be empty.
    pub short_desc: String,
}

impl RemoteListing {
    /// `(source_id, id)` - globally unique within the launcher's
    /// universe of catalogs.
    #[must_use]
    pub fn key(&self) -> (&str, &str) {
        (self.source_id.as_str(), self.id.as_str())
    }
}

/// Async catalog interface.
///
/// Cheap to clone (each impl uses an `Arc` internally), so callers
/// can keep a single instance and hand clones into spawned tasks.
#[async_trait::async_trait]
pub trait RemoteCatalog: Send + Sync {
    /// Stable id of this catalog (used in cache keys).
    fn id(&self) -> &'static str;

    /// Human-readable label for UI chips.
    fn label(&self) -> &'static str;

    /// Fetch every published listing for `game_filter`. Catalogs that
    /// paginate must walk all pages internally so callers see one
    /// flat list.
    async fn list(&self, game_filter: GameFilter) -> Result<Vec<RemoteListing>, ClientError>;

    /// Fetch the full mod record for one listing - the heavy fields
    /// (description, dependencies, download URL) the listing call
    /// doesn't include. Implementations may return cached data if
    /// the listing's `updated_at` matches the cache.
    async fn fetch_full(&self, listing: &RemoteListing) -> Result<Mod, ClientError>;
}

/// Filter applied to a [`RemoteCatalog::list`] call.
#[derive(Debug, Clone)]
pub enum GameFilter {
    /// Catalog-specific game id. MW uses an integer; Nexus uses a
    /// short slug; GitHub doesn't really have one.
    GameId(String),
}

/// ModWorkshop catalog. Wraps the existing [`Client`] so the same
/// dedupe + on-disk cache layer fronts both the listing and per-mod
/// paths.
#[derive(Clone)]
pub struct MwCatalog {
    client: Arc<Client>,
}

impl MwCatalog {
    /// Build a fresh catalog wrapping the supplied client. Callers
    /// can also use [`MwCatalog::default`] to spawn a brand-new
    /// client if they don't have one yet.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client: Arc::new(client) }
    }

    /// Convenience accessor: needed by the launcher when doing
    /// something the trait doesn't expose yet (e.g. fetching
    /// images, dep-name lookups).
    #[must_use]
    pub fn client(&self) -> &Client {
        &self.client
    }
}

impl Default for MwCatalog {
    fn default() -> Self {
        Self::new(Client::new())
    }
}

#[async_trait::async_trait]
impl RemoteCatalog for MwCatalog {
    fn id(&self) -> &'static str {
        "modworkshop"
    }

    fn label(&self) -> &'static str {
        "ModWorkshop"
    }

    async fn list(&self, game_filter: GameFilter) -> Result<Vec<RemoteListing>, ClientError> {
        let GameFilter::GameId(game_id_str) = game_filter;
        let game_id: u64 = game_id_str
            .parse()
            .map_err(|e| ClientError::Parse(format!("game_id parse: {e}")))?;
        self.client.list_game_mods(game_id).await
    }

    async fn fetch_full(&self, listing: &RemoteListing) -> Result<Mod, ClientError> {
        let id: u64 = listing
            .id
            .parse()
            .map_err(|e| ClientError::Parse(format!("listing id parse: {e}")))?;
        self.client.get_mod(id).await
    }
}

/// Helper to convert a parsed [`Mod`] (from listing or detail) into
/// the trimmed [`RemoteListing`] view. Used internally by [`Client::list_game_mods`]
/// but also exposed for ad-hoc external mapping.
#[must_use]
pub fn listing_from_mod(m: &Mod) -> RemoteListing {
    let author = m
        .user_inline
        .as_ref()
        .map(|u| u.name.clone())
        .unwrap_or_default();
    let author_avatar = m
        .user_inline
        .as_ref()
        .map(|u| u.avatar.clone())
        .unwrap_or_default();
    RemoteListing {
        id: m.id.to_string(),
        name: m.name.clone(),
        source_id: "modworkshop".to_string(),
        author,
        author_avatar,
        version: m.version.clone(),
        downloads: m.downloads,
        likes: m.likes,
        views: m.views,
        thumbnail: m.thumbnail.clone(),
        tags: m.tags.clone(),
        updated_at: m.updated_at.clone(),
        short_desc: m.short_desc.clone(),
    }
}

// `User` is re-exported so the trait module reads cleanly even
// though the listing carries the user fields inline.
#[allow(dead_code)]
pub(crate) type _UserRef = User;
