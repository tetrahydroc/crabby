//! Bake-time injection of an id-index field onto vanilla `TraderData.gd`.
//!
//! `TraderData` is the shared schema for the four vanilla trader
//! resources (`Generalist.tres`, `Doctor.tres`, `Gunsmith.tres`, ...).
//! Each has a `@export var tasks: Array[TaskData]` field plus voice /
//! tax / etc. metadata.
//!
//! Injecting onto the shared class means every TraderData instance gets
//! its own per-instance `_id_index` of TaskData entries, populated at
//! `_init` from the `tasks` array. That gives lib's trader_tasks
//! registry per-trader-keyed lookup of vanilla tasks. Mod-registered
//! tasks live under their mod-supplied id in the same index.
//!
//! Id derivation: file-stem of `task.resource_path`. Vanilla tasks live
//! at paths like `res://Traders/Generalist/Task_X.tres`; the stem
//! (`Task_X`) is the natural id.

use crate::id_index_transform::{
    IdIndexConfig, IdSource, IndexShape, transform as id_index_transform,
};

/// Filename of the shared TraderData schema script.
pub const TRADER_DATA_SCHEMA_FILENAME: &str = "TraderData.gd";

const CONFIG: IdIndexConfig = IdIndexConfig {
    class_name: "TraderData",
    element_type: "TaskData",
    shape: IndexShape::Single { field: "tasks" },
    id_source: IdSource::ResourcePathStem,
};

/// Inject id-index machinery if `filename` matches; otherwise pass
/// through unchanged.
#[must_use]
pub fn transform(filename: &str, source: &str) -> String {
    if filename != TRADER_DATA_SCHEMA_FILENAME {
        return source.to_string();
    }
    id_index_transform(source, &CONFIG)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VANILLA_TRADER_DATA: &str = "extends Resource\nclass_name TraderData\n\n\
        @export var icon: Texture2D\n\
        @export var name: String\n\
        @export var tasks: Array[TaskData]\n";

    #[test]
    fn appends_index_block_keyed_by_path_stem() {
        let out = transform(TRADER_DATA_SCHEMA_FILENAME, VANILLA_TRADER_DATA);
        assert!(out.contains("var _id_index: Dictionary = {}"), "{out}");
        assert!(out.contains("for r in tasks:"), "{out}");
        assert!(out.contains("get_basename"), "{out}");
        assert!(
            out.contains("func _index_add(id: String, entry: TaskData)"),
            "{out}"
        );
    }

    #[test]
    fn skips_when_filename_doesnt_match() {
        let out = transform("OtherScript.gd", VANILLA_TRADER_DATA);
        assert_eq!(out, VANILLA_TRADER_DATA);
    }
}
