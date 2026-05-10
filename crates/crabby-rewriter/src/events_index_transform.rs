//! Bake-time injection of an id-index field onto vanilla `Events.gd`.
//!
//! Single typed-Array `events: Array[EventData]`. Index value is the
//! `EventData` Resource directly, keyed by file-stem. See
//! `id_index_transform` for the underlying machinery.

use crate::id_index_transform::{IdIndexConfig, IdSource, IndexShape, transform as id_index_transform};

/// Filename of vanilla's Events resource script.
pub const EVENTS_SCHEMA_FILENAME: &str = "Events.gd";

const CONFIG: IdIndexConfig = IdIndexConfig {
    class_name: "Events",
    element_type: "EventData",
    shape: IndexShape::Single { field: "events" },
    id_source: IdSource::ResourcePathStem,
};

/// Inject id-index machinery if `filename` matches; otherwise pass
/// through unchanged.
#[must_use]
pub fn transform(filename: &str, source: &str) -> String {
    if filename != EVENTS_SCHEMA_FILENAME {
        return source.to_string();
    }
    id_index_transform(source, &CONFIG)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VANILLA_EVENTS: &str =
        "extends Resource\nclass_name Events\n@export var events: Array[EventData]\n";

    #[test]
    fn appends_single_index_block() {
        let out = transform(EVENTS_SCHEMA_FILENAME, VANILLA_EVENTS);
        assert!(out.contains("var _id_index: Dictionary = {}"), "{out}");
        assert!(out.contains("for r in events:"), "{out}");
        assert!(out.contains("func _index_add(id: String, entry: EventData)"), "{out}");
    }

    #[test]
    fn skips_when_filename_doesnt_match() {
        let out = transform("OtherScript.gd", VANILLA_EVENTS);
        assert_eq!(out, VANILLA_EVENTS);
    }
}
