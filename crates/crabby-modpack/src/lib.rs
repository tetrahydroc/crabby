//! Shareable mod-set + MCM bundle format ("modpack").
//!
//! A modpack carries:
//! - The active profile's mod list (id, version, optional MW id) as
//!   the recipient should reproduce it.
//! - Per-mod MCM config snapshots - raw `config.ini` text bytes,
//!   one per mod that has one on disk.
//!
//! # Two carrier formats
//!
//! - **String code:** `crabby:pack:<schema>:<base64-zstd-json>`. Self-
//!   contained, paste-friendly for small packs (~30 mods stays under
//!   ~20KB encoded). Greppable in chat logs via the `crabby:pack:`
//!   prefix.
//! - **File:** `<name>.crabbypack` - same payload, no base64 wrapper,
//!   just `<schema>\n<zstd-bytes>`. Better for big packs / sharing
//!   via attachment.
//!
//! # Schema versioning
//!
//! `Manifest::SCHEMA_VERSION` is bumped on any breaking change. Older
//! schemas are not back-imported in v1; a clear error surfaces and
//! the recipient is asked to upgrade the source crabby. (The schema
//! is broad enough that migrations aren't expected soon.)

#![deny(missing_docs)]

use std::path::Path;

use base64::Engine;
use serde::{Deserialize, Serialize};

/// One mod in the pack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackMod {
    /// Local mod id (the manifest's `[mod] id`). Recipient matches by
    /// this when checking "do I already have it installed".
    pub id: String,
    /// Display name as seen by the source; surfaced in the import
    /// preview so the recipient sees friendly names, not just ids.
    pub name: String,
    /// Pack-author's installed version. Recipient compares; mismatched
    /// versions trigger a "keep installed" decision in the import flow.
    pub version: String,
    /// ModWorkshop numeric id, when known. `None` for non-MW mods
    /// (folder mods, off-MW vmz files). Non-MW entries surface in the
    /// import preview as "skipped, install manually" but stay in the
    /// pack so the recipient at least knows which mods to find.
    #[serde(default)]
    pub mw_id: Option<u64>,
    /// MCM config bytes, when the mod had one on disk at export time.
    /// Stored as raw `config.ini` text bytes; the import flow drops
    /// them at `<user>/MCM/<id>/config.ini` directly.
    #[serde(default)]
    pub mcm_config: Option<Vec<u8>>,
}

/// Top-level pack manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Schema version. Bump on breaking changes.
    pub schema: u32,
    /// Pack name; defaults to the source profile name.
    pub name: String,
    /// Optional human description.
    #[serde(default)]
    pub description: String,
    /// Source-side crabby version (`env!("CARGO_PKG_VERSION")`) for
    /// debugging "this pack was made by X version".
    pub crabby_version: String,
    /// Unix-seconds export timestamp.
    pub created_at: u64,
    /// Mods in the pack, in display order.
    pub mods: Vec<PackMod>,
}

impl Manifest {
    /// Current schema version. Bump on any breaking format change.
    pub const SCHEMA_VERSION: u32 = 1;
}

/// Magic prefix on the string-code form. Lets people grep for shared
/// codes in chat logs.
pub const STRING_PREFIX: &str = "crabby:pack:";

/// Suggested file extension for the file-form pack.
pub const FILE_EXTENSION: &str = "crabbypack";

/// File header line; written before the zstd payload so a quick
/// `head -1` shows the schema. Same shape as the string code's middle
/// segment for symmetry.
const FILE_HEADER: &str = "crabby:pack:";

/// Errors from encode/decode.
#[derive(Debug, thiserror::Error)]
pub enum ModpackError {
    /// Schema mismatch: pack was made by a newer (or much older)
    /// crabby than the recipient.
    #[error("modpack schema {got} not supported (this crabby reads schema {expected})")]
    UnsupportedSchema {
        /// Schema we expected.
        expected: u32,
        /// Schema we found.
        got: u32,
    },
    /// String code didn't start with [`STRING_PREFIX`].
    #[error("string is not a crabby pack code (no `{STRING_PREFIX}` prefix)")]
    NotAPack,
    /// String code's parts weren't `prefix:schema:body`.
    #[error("malformed pack code")]
    Malformed,
    /// File-form pack didn't start with the expected header.
    #[error("not a crabby pack file (missing header)")]
    NotAPackFile,
    /// Base64 decode error.
    #[error("base64 decode: {0}")]
    Base64(#[from] base64::DecodeError),
    /// JSON decode error.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// IO error (file form only).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Underlying zstd error.
    #[error("zstd: {0}")]
    Zstd(String),
}

/// Encode a manifest into the string-code form: `crabby:pack:<schema>:<base64-zstd-json>`.
pub fn encode_string(m: &Manifest) -> Result<String, ModpackError> {
    let json = serde_json::to_vec(m)?;
    let compressed = zstd::stream::encode_all(json.as_slice(), 19)
        .map_err(|e| ModpackError::Zstd(e.to_string()))?;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&compressed);
    Ok(format!("{STRING_PREFIX}{}:{}", m.schema, b64))
}

/// Decode a string-code into a manifest. Validates the schema before
/// returning.
pub fn decode_string(code: &str) -> Result<Manifest, ModpackError> {
    let trimmed = code.trim();
    let body = trimmed
        .strip_prefix(STRING_PREFIX)
        .ok_or(ModpackError::NotAPack)?;
    let (schema_str, b64) = body.split_once(':').ok_or(ModpackError::Malformed)?;
    let schema: u32 = schema_str.parse().map_err(|_| ModpackError::Malformed)?;
    if schema != Manifest::SCHEMA_VERSION {
        return Err(ModpackError::UnsupportedSchema {
            expected: Manifest::SCHEMA_VERSION,
            got: schema,
        });
    }
    let compressed = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(b64.as_bytes())?;
    let json = zstd::stream::decode_all(compressed.as_slice())
        .map_err(|e| ModpackError::Zstd(e.to_string()))?;
    let m: Manifest = serde_json::from_slice(&json)?;
    Ok(m)
}

/// Encode a manifest into the file-form bytes (`<header><schema>\n<zstd-bytes>`).
pub fn encode_file_bytes(m: &Manifest) -> Result<Vec<u8>, ModpackError> {
    let json = serde_json::to_vec(m)?;
    let compressed = zstd::stream::encode_all(json.as_slice(), 19)
        .map_err(|e| ModpackError::Zstd(e.to_string()))?;
    let header = format!("{FILE_HEADER}{}\n", m.schema);
    let mut out = header.into_bytes();
    out.extend_from_slice(&compressed);
    Ok(out)
}

/// Write a pack to a file. Convenience wrapper over [`encode_file_bytes`].
pub fn write_pack_file(path: &Path, m: &Manifest) -> Result<(), ModpackError> {
    let bytes = encode_file_bytes(m)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Decode file-form bytes (`<header><schema>\n<zstd-bytes>`).
pub fn decode_file_bytes(bytes: &[u8]) -> Result<Manifest, ModpackError> {
    // Header line ends at the first newline.
    let nl = bytes
        .iter()
        .position(|&b| b == b'\n')
        .ok_or(ModpackError::NotAPackFile)?;
    let header = std::str::from_utf8(&bytes[..nl]).map_err(|_| ModpackError::NotAPackFile)?;
    let schema_str = header
        .strip_prefix(FILE_HEADER)
        .ok_or(ModpackError::NotAPackFile)?;
    let schema: u32 = schema_str.parse().map_err(|_| ModpackError::NotAPackFile)?;
    if schema != Manifest::SCHEMA_VERSION {
        return Err(ModpackError::UnsupportedSchema {
            expected: Manifest::SCHEMA_VERSION,
            got: schema,
        });
    }
    let compressed = &bytes[nl + 1..];
    let json =
        zstd::stream::decode_all(compressed).map_err(|e| ModpackError::Zstd(e.to_string()))?;
    let m: Manifest = serde_json::from_slice(&json)?;
    Ok(m)
}

/// Read a pack from a file. Convenience wrapper.
pub fn read_pack_file(path: &Path) -> Result<Manifest, ModpackError> {
    let bytes = std::fs::read(path)?;
    decode_file_bytes(&bytes)
}

/// Auto-detect input shape: tries the string-code form first (the
/// common paste case), falls back to file-bytes form. Useful for the
/// import textbox which accepts either.
pub fn decode_any(input: &[u8]) -> Result<Manifest, ModpackError> {
    // String form is ASCII; if the bytes are valid UTF-8 starting with
    // the string prefix, parse as a string code.
    if let Ok(s) = std::str::from_utf8(input) {
        let trimmed = s.trim_start();
        if trimmed.starts_with(STRING_PREFIX) {
            return decode_string(trimmed);
        }
    }
    decode_file_bytes(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> Manifest {
        Manifest {
            schema: Manifest::SCHEMA_VERSION,
            name: "test pack".into(),
            description: String::new(),
            crabby_version: "0.1.0".into(),
            created_at: 1_777_000_000,
            mods: vec![
                PackMod {
                    id: "hold-breath".into(),
                    name: "Hold Breath".into(),
                    version: "1.0.3".into(),
                    mw_id: Some(49779),
                    mcm_config: Some(b"[main]\nbinding=\"shift\"\n".to_vec()),
                },
                PackMod {
                    id: "folder-only".into(),
                    name: "Folder Only".into(),
                    version: "0.1.0".into(),
                    mw_id: None,
                    mcm_config: None,
                },
            ],
        }
    }

    #[test]
    fn string_round_trip() {
        let m = sample_manifest();
        let s = encode_string(&m).unwrap();
        assert!(s.starts_with(STRING_PREFIX), "{s}");
        let back = decode_string(&s).unwrap();
        assert_eq!(back.name, m.name);
        assert_eq!(back.mods.len(), 2);
        assert_eq!(back.mods[0].mw_id, Some(49779));
        assert_eq!(back.mods[1].mw_id, None);
    }

    #[test]
    fn file_round_trip() {
        let m = sample_manifest();
        let bytes = encode_file_bytes(&m).unwrap();
        assert!(bytes.starts_with(FILE_HEADER.as_bytes()));
        let back = decode_file_bytes(&bytes).unwrap();
        assert_eq!(
            back.mods[0].mcm_config.as_deref(),
            Some(&b"[main]\nbinding=\"shift\"\n"[..])
        );
    }

    #[test]
    fn decode_any_handles_both() {
        let m = sample_manifest();
        let s = encode_string(&m).unwrap();
        assert_eq!(decode_any(s.as_bytes()).unwrap().name, m.name);
        let bytes = encode_file_bytes(&m).unwrap();
        assert_eq!(decode_any(&bytes).unwrap().name, m.name);
    }

    #[test]
    fn rejects_wrong_prefix() {
        let err = decode_string("not-a-pack:foo").unwrap_err();
        assert!(matches!(err, ModpackError::NotAPack), "{err:?}");
    }

    #[test]
    fn rejects_unknown_schema() {
        let m = Manifest {
            schema: 999,
            ..sample_manifest()
        };
        // Encode with the bad schema present in the manifest, then
        // decode; decoder rejects because schema != SCHEMA_VERSION.
        let s = encode_string(&m).unwrap();
        let err = decode_string(&s).unwrap_err();
        assert!(
            matches!(err, ModpackError::UnsupportedSchema { .. }),
            "{err:?}"
        );
    }
}
