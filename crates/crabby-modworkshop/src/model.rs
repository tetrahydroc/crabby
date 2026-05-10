//! Typed shapes for the ModWorkshop API responses we consume.
//!
//! The spec doesn't publish these, so the field set here is what was
//! observed live. `#[serde(default)]` on every field means the
//! parser tolerates partial/missing data; if MW adds new fields they
//! are ignored, if MW removes one the default kicks in.

use serde::{Deserialize, Deserializer, Serialize};

/// Treat JSON `null` as `Default::default()`. The MW API returns
/// `null` for many optional fields (repo_url, allowed_storage, etc.)
/// rather than omitting them, and serde's `#[serde(default)]` only
/// covers *missing* keys; `null` still hits the field's deserializer
/// and fails on non-`Option` types.
fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Option::<T>::deserialize(d).map(Option::unwrap_or_default)
}

/// One mod's metadata. Mirrors the `GET /mods/{id}` response shape.
///
/// Most fields are optional in practice (different mods omit
/// different things), so everything is `default`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Mod {
    /// MW numeric id.
    #[serde(default, deserialize_with = "null_default")]
    pub id: u64,
    /// Display name.
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    /// Long markdown description.
    #[serde(default, deserialize_with = "null_default")]
    pub desc: String,
    /// Short tagline shown in lists.
    #[serde(default, deserialize_with = "null_default")]
    pub short_desc: String,
    /// Author/owner numeric id. Use [`crate::Client::user`] to resolve
    /// the display name.
    #[serde(default, deserialize_with = "null_default")]
    pub user_id: u64,
    /// Inline author record. Both detail and listing responses
    /// include this; `user_id` is kept separately because some
    /// historical entries had it without the inline object.
    #[serde(default, rename = "user")]
    pub user_inline: Option<User>,
    /// Most-recent edit timestamp (ISO-8601). Drives stale-cache
    /// detection: when this advances past the cached value the
    /// per-mod record is invalidated.
    #[serde(default, deserialize_with = "null_default")]
    pub updated_at: String,
    /// Author-declared current version. Mirrored on the file too;
    /// these almost always agree but this field is the source of
    /// truth for the "header" version display.
    #[serde(default, deserialize_with = "null_default")]
    pub version: String,
    /// Lifetime download count.
    #[serde(default, deserialize_with = "null_default")]
    pub downloads: u64,
    /// Like count.
    #[serde(default, deserialize_with = "null_default")]
    pub likes: u64,
    /// View count.
    #[serde(default, deserialize_with = "null_default")]
    pub views: u64,
    /// ISO-8601 timestamp of the last "bump" (publish or major edit).
    /// Use this for "last updated" UI.
    #[serde(default, deserialize_with = "null_default")]
    pub bumped_at: String,
    /// First-publish timestamp.
    #[serde(default, deserialize_with = "null_default")]
    pub published_at: String,
    /// Source repo (GitHub etc.) when the author linked one.
    #[serde(default, deserialize_with = "null_default")]
    pub repo_url: String,
    /// Either `"public"`, `"private"`, or `"unlisted"`.
    #[serde(default, deserialize_with = "null_default")]
    pub visibility: String,
    /// Suspended-by-MW flag. Hide these from update suggestions.
    #[serde(default, deserialize_with = "null_default")]
    pub suspended: bool,
    /// Latest download metadata (file + version). Sometimes null even
    /// when the mod actually has files; see `has_download` and the
    /// `/files` endpoint for the multi-file fallback.
    #[serde(default)]
    pub download: Option<ModDownload>,
    /// MW's "this mod has at least one downloadable file" flag. When
    /// `download` is None but this is true, fall back to `/files`.
    #[serde(default, deserialize_with = "null_default")]
    pub has_download: bool,
    /// MW-curated tags. Drives the tag chips below the title.
    #[serde(default)]
    pub tags: Vec<Tag>,
    /// Declared dependencies. Each entry's `mod_id` points at the
    /// MW page of the required mod; full records are not fetched
    /// up-front. The UI links to the page and lets the user follow.
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    /// Square-ish thumbnail used in lists/cards. None when the author
    /// hasn't uploaded one.
    #[serde(default)]
    pub thumbnail: Option<Image>,
    /// Wide hero banner. None when the author hasn't uploaded one.
    #[serde(default)]
    pub banner: Option<Image>,
    /// Low-opacity wash for the detail page background.
    #[serde(default)]
    pub background: Option<Image>,
    /// Gallery: author-uploaded screenshots. Sorted client-side by
    /// `display_order`; the API returns them already roughly sorted
    /// but upstream ordering is not trusted.
    #[serde(default)]
    pub images: Vec<Image>,
}

/// One image record returned by the MW API. The `file` field is the
/// CDN-relative filename - pair with [`crate::image_url`] to get a
/// fetchable URL. Dimensions aren't modeled because the API doesn't
/// return them; the renderer queries the decoded bytes instead.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Image {
    /// MW numeric image id.
    #[serde(default, deserialize_with = "null_default")]
    pub id: u64,
    /// Storage filename (e.g. `"abc123.webp"`).
    #[serde(default, deserialize_with = "null_default")]
    pub file: String,
    /// File extension category (`webp`, `png`, `jpg`).
    #[serde(default, deserialize_with = "null_default", rename = "type")]
    pub kind: String,
    /// Bytes.
    #[serde(default, deserialize_with = "null_default")]
    pub size: u64,
    /// Whether MW has generated a smaller thumb variant.
    #[serde(default, deserialize_with = "null_default")]
    pub has_thumb: bool,
    /// Author-set order in the gallery. Sort ascending for display.
    #[serde(default, deserialize_with = "null_default")]
    pub display_order: i32,
    /// Whether the image is publicly visible.
    #[serde(default, deserialize_with = "null_default")]
    pub visible: bool,
}

/// One MW tag.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Tag {
    /// MW numeric id.
    #[serde(default, deserialize_with = "null_default")]
    pub id: u64,
    /// Display label (e.g. "Quality of Life").
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    /// CSS-style hex color (`"#369649"`). May be empty.
    #[serde(default, deserialize_with = "null_default")]
    pub color: String,
}

/// One dependency declaration on a mod.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Dependency {
    /// Dependency record id (not the dep mod's id).
    #[serde(default, deserialize_with = "null_default")]
    pub id: u64,
    /// MW numeric id of the required mod. The link target.
    #[serde(default, deserialize_with = "null_default")]
    pub mod_id: u64,
    /// Author-supplied display name. Often `null` for in-MW deps;
    /// the UI falls back to "mod #N" or fetches the real name.
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    /// Off-site URL when the dep isn't on MW (rare). Empty when MW.
    #[serde(default, deserialize_with = "null_default")]
    pub url: String,
    /// Whether the dep is optional vs required.
    #[serde(default, deserialize_with = "null_default")]
    pub optional: bool,
    /// `"mod"` for normal mod deps; future MW kinds may differ.
    #[serde(default, deserialize_with = "null_default", rename = "dependable_type")]
    pub kind: String,
    /// Display order within the mod's dependency list.
    #[serde(default, deserialize_with = "null_default")]
    pub order: i32,
}

/// One file/download attached to a mod. Version is surfaced
/// for update detection.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModDownload {
    /// MW numeric id of the download record.
    #[serde(default, deserialize_with = "null_default")]
    pub id: u64,
    /// File extension category (`vmz`, `zip`, etc.).
    #[serde(default, rename = "type", deserialize_with = "null_default")]
    pub kind: String,
    /// Author-declared file version. Often agrees with `Mod::version`
    /// but isn't guaranteed.
    #[serde(default, deserialize_with = "null_default")]
    pub version: String,
    /// Bytes.
    #[serde(default, deserialize_with = "null_default")]
    pub size: u64,
    /// Direct download URL.
    #[serde(default, deserialize_with = "null_default")]
    pub download_url: String,
}

/// A MW user - only the displayed fields.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct User {
    /// MW numeric id.
    #[serde(default, deserialize_with = "null_default")]
    pub id: u64,
    /// Display name.
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    /// Slugified handle, used in MW URLs.
    #[serde(default, deserialize_with = "null_default")]
    pub unique_name: String,
    /// Avatar filename (relative to MW's CDN).
    #[serde(default, deserialize_with = "null_default")]
    pub avatar: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_string_fields_deserialize_to_empty() {
        // MW returns explicit `null` for missing optional strings.
        // Without the null_default deserializer this fails with
        // "invalid type: null, expected a string".
        let json = r#"{
            "id": 56284,
            "name": "GunsmithsBunker",
            "user_id": 1,
            "version": "1.1.0",
            "repo_url": null,
            "downloads": 294,
            "likes": 7,
            "views": 24,
            "bumped_at": "2026-04-29T11:08:32.000000Z",
            "published_at": "2026-04-08T19:08:55.000000Z",
            "visibility": "public",
            "suspended": false,
            "download": null
        }"#;
        let m: Mod = serde_json::from_str(json).expect("null fields parse");
        assert_eq!(m.id, 56284);
        assert_eq!(m.repo_url, "");
        assert!(m.download.is_none());
    }
}
