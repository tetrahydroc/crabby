//! Bake-time injection of an id-index field onto vanilla `LootTable.gd`.
//!
//! `LootTable` is the shared schema for **every** loot table on disk:
//! `LT_Master.tres` (the canonical item catalog) plus the kit / area /
//! tutorial tables. Each has `@export var items: Array[ItemData]`.
//!
//! Injecting onto the shared class means every LootTable instance gets
//! its own per-instance `_id_index`, built lazily on first access (see
//! `id_index_transform` for why not at `_init`). That covers:
//!
//! - **Items registry** (LT_Master), `lib.has("items", "AKM")` reads
//!   `LT_Master._id_index["AKM"]` instead of walking the array.
//! - **Loot registry** (any LT_*), per-table queries fall out for free
//!   (`some_loot_table._id_index.has(item.file)`); not exposed via lib's
//!   read API today, but the data is there if needed later.
//!
//! Id derivation: `it.file` (the canonical ItemData id field), not the
//! `.tres` path stem. Vanilla items are organized by file paths that
//! don't match their canonical names directly (e.g. AK-12.tres is
//! addressed as "AK-12" via its `.file` field).

use crate::id_index_transform::{
    IdIndexConfig, IdSource, IndexShape, transform as id_index_transform,
};

/// Filename of the shared LootTable schema script.
pub const LOOT_TABLE_SCHEMA_FILENAME: &str = "LootTable.gd";

const CONFIG: IdIndexConfig = IdIndexConfig {
    class_name: "LootTable",
    element_type: "ItemData",
    shape: IndexShape::Single { field: "items" },
    id_source: IdSource::Field { name: "file" },
};

/// Inject id-index machinery if `filename` matches; otherwise pass
/// through unchanged.
#[must_use]
pub fn transform(filename: &str, source: &str) -> String {
    if filename != LOOT_TABLE_SCHEMA_FILENAME {
        return source.to_string();
    }
    id_index_transform(source, &CONFIG)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VANILLA_LOOT_TABLE: &str =
        "extends Resource\nclass_name LootTable\n\n@export var items: Array[ItemData]\n";

    #[test]
    fn appends_index_block_keyed_by_file() {
        let out = transform(LOOT_TABLE_SCHEMA_FILENAME, VANILLA_LOOT_TABLE);
        // Lazy id-index: backing storage + property getter, no `_init`.
        assert!(
            out.contains("var _id_index_storage: Dictionary = {}"),
            "{out}"
        );
        assert!(out.contains("var _id_index: Dictionary:"), "{out}");
        assert!(!out.contains("func _init"), "{out}");
        assert!(out.contains("for r in items:"), "{out}");
        assert!(out.contains(r#"var v: Variant = r.get("file")"#), "{out}");
        assert!(
            out.contains("func _index_add(id: String, entry: ItemData)"),
            "{out}"
        );
    }

    #[test]
    fn skips_when_filename_doesnt_match() {
        let out = transform("OtherScript.gd", VANILLA_LOOT_TABLE);
        assert_eq!(out, VANILLA_LOOT_TABLE);
    }
}
