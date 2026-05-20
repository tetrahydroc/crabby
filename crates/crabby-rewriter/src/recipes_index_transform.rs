//! Bake-time injection of an id-index field onto vanilla `Recipes.gd`.
//!
//! Vanilla `Recipes.gd` declares 7 typed-Array categories
//! (`consumables`, `medical`, `equipment`, `weapons`, `electronics`,
//! `misc`, `furniture`). The injected index is keyed by the file-stem
//! of each recipe's `resource_path`; values are
//! `{"recipe": RecipeData, "category": String}` so callers know which
//! Array to mutate when registering / removing.
//!
//! See `id_index_transform` for the underlying machinery and per-kind
//! sibling modules (`events_index_transform`, etc.) for analogous
//! single-array kinds.

use crate::id_index_transform::{
    IdIndexConfig, IdSource, IndexShape, transform as id_index_transform,
};

/// Filename of vanilla's Recipes resource script.
pub const RECIPES_SCHEMA_FILENAME: &str = "Recipes.gd";

const CONFIG: IdIndexConfig = IdIndexConfig {
    class_name: "Recipes",
    element_type: "RecipeData",
    shape: IndexShape::Categorized {
        fields: &[
            "consumables",
            "medical",
            "equipment",
            "weapons",
            "electronics",
            "misc",
            "furniture",
        ],
    },
    id_source: IdSource::ResourcePathStem,
};

/// Inject id-index machinery if `filename` matches and the source has
/// the expected vanilla shape; otherwise pass through unchanged.
#[must_use]
pub fn transform(filename: &str, source: &str) -> String {
    if filename != RECIPES_SCHEMA_FILENAME {
        return source.to_string();
    }
    id_index_transform(source, &CONFIG)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VANILLA_RECIPES: &str = "extends Resource\nclass_name Recipes\n\n\
        @export var consumables: Array[RecipeData]\n\
        @export var medical: Array[RecipeData]\n\
        @export var equipment: Array[RecipeData]\n\
        @export var weapons: Array[RecipeData]\n\
        @export var electronics: Array[RecipeData]\n\
        @export var misc: Array[RecipeData]\n\
        @export var furniture: Array[RecipeData]\n";

    #[test]
    fn appends_categorized_index_block() {
        let out = transform(RECIPES_SCHEMA_FILENAME, VANILLA_RECIPES);
        assert!(
            out.contains("var _id_index_storage: Dictionary = {}"),
            "{out}"
        );
        assert!(out.contains("var _id_index: Dictionary:"), "{out}");
        assert!(!out.contains("func _init"), "{out}");
        assert!(out.contains(r#"for cat in ["consumables", "medical", "equipment", "weapons", "electronics", "misc", "furniture"]:"#), "{out}");
        assert!(
            out.contains("func _index_add(id: String, entry: RecipeData, category: String)"),
            "{out}"
        );
    }

    #[test]
    fn skips_when_filename_doesnt_match() {
        let out = transform("OtherScript.gd", VANILLA_RECIPES);
        assert_eq!(out, VANILLA_RECIPES);
    }

    #[test]
    fn skips_when_shape_doesnt_match() {
        let src = "extends Resource\n@export var weapons: Array[RecipeData]\n";
        let out = transform(RECIPES_SCHEMA_FILENAME, src);
        assert_eq!(out, src);
    }
}
