//! Generic bake-time injection of an id-index field onto a vanilla
//! data Resource script.
//!
//! Per-kind transforms (recipes, events, sounds, fish, ...) describe
//! their target via [`IdIndexConfig`] and call [`transform`]. The
//! transform emits a uniform shape:
//!
//! - `var _id_index: Dictionary = {}`, runtime lookup
//! - `func _init() -> void: _rebuild_id_index()`
//! - `func _rebuild_id_index()` walks the source field(s) and indexes
//!   by `_derive_id(resource)`
//! - `func _derive_id(r: Resource) -> String`, file-stem of resource_path
//! - `func _index_add / _index_remove / _index_set`, maintenance helpers
//!
//! Two shapes are supported:
//!
//! - [`IndexShape::Single`], one typed-Array field. Index value is the
//!   Resource directly. Helpers take `(id, resource)`.
//! - [`IndexShape::Categorized`], multiple typed-Array fields, one per
//!   category. Index value is `{resource, category}` so callers know
//!   which Array to mutate. Helpers take `(id, resource, category)`.
//!
//! See [`recipes_index_transform`] and [`events_index_transform`] for
//! per-kind glue.

/// Per-kind configuration for the transform. Fully describes the shape
/// expected on the vanilla schema script, the fields that hold the
/// canonical typed-Array data, and the names emitted into the injected
/// block.
pub struct IdIndexConfig {
    /// Vanilla `class_name` declared at the top of the schema script.
    /// Used both as a shape sanity check and embedded in the injected
    /// block's banner comment for readability.
    pub class_name: &'static str,
    /// Element type inside the typed-Array(s), e.g. `RecipeData`,
    /// `EventData`. Used to verify the source `@export var x:
    /// Array[<type>]` declaration exists before any mutation.
    pub element_type: &'static str,
    /// Shape of the source data: single typed-Array or N categorized
    /// typed-Arrays.
    pub shape: IndexShape,
    /// How `_derive_id` reads the id from each Resource entry. Default
    /// is file-stem of `resource_path`; some kinds (items / ItemData)
    /// carry their own canonical id field.
    pub id_source: IdSource,
}

/// Single typed-Array vs. multiple categorized arrays.
pub enum IndexShape {
    /// One typed-Array field. Helpers index by id only.
    Single { field: &'static str },
    /// Multiple typed-Array fields, one per category. Helpers index by
    /// (id, category) so callers can route writes back to the right
    /// array.
    Categorized { fields: &'static [&'static str] },
}

/// Where the id comes from for each Resource entry in the typed Array.
pub enum IdSource {
    /// `r.resource_path.get_file().get_basename()`, file-stem of the
    /// `.tres` path. Stable, unique per file. Used when the schema
    /// itself doesn't carry an id field.
    ResourcePathStem,
    /// `r.get(field_name)`, read a String-typed field from the Resource
    /// itself. Used when the schema already has a canonical id
    /// (e.g. `ItemData.file`).
    Field { name: &'static str },
}

/// Inject id-index machinery at the end of `source` if the file matches
/// `cfg`'s expected shape. Otherwise return `source` unchanged.
///
/// Idempotent: if the source already has `func _init` or
/// `var _id_index`, returns unchanged. The shape check ensures a future
/// vanilla schema change breaks loud (transform skips, downstream
/// consumers fail at runtime) rather than corrupting the script.
#[must_use]
pub fn transform(source: &str, cfg: &IdIndexConfig) -> String {
    if !looks_like_vanilla(source, cfg) {
        return source.to_string();
    }
    // Idempotency: if the script already carries the injected index
    // (its storage var, or a leftover `func _init` from the old
    // eager-build shape), don't re-inject. A future vanilla schema that
    // happens to declare its own `_init` also short-circuits here, which
    // is the safe choice — better to skip than to corrupt.
    if source.contains("_id_index_storage")
        || source.contains("var _id_index")
        || source.contains("func _init")
    {
        return source.to_string();
    }

    let mut out = source.trim_end().to_string();
    out.push_str("\n\n");
    out.push_str(&render_block(cfg));
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Sanity-check that `source` is the expected vanilla shape: declares
/// the right `class_name` and has the typed-Array `@export var` fields
/// the config names.
fn looks_like_vanilla(source: &str, cfg: &IdIndexConfig) -> bool {
    let class_decl = format!("class_name {}", cfg.class_name);
    if !source.contains(&class_decl) {
        return false;
    }
    match cfg.shape {
        IndexShape::Single { field } => {
            let needle = format!("@export var {}: Array[{}]", field, cfg.element_type);
            source.contains(&needle)
        }
        IndexShape::Categorized { fields } => fields.iter().all(|f| {
            let needle = format!("@export var {}: Array[{}]", f, cfg.element_type);
            source.contains(&needle)
        }),
    }
}

/// Build the injected block for `cfg`. Output is tab-indented to match
/// vanilla's indent style across the RTV Scripts/ corpus.
fn render_block(cfg: &IdIndexConfig) -> String {
    let mut s = String::with_capacity(1024);
    s.push_str("# --- crabby id-index (injected at bake) -------------------------------\n");
    s.push_str("# Runtime-only lookup: ");
    match cfg.shape {
        IndexShape::Single { .. } => {
            s.push_str("id -> ");
            s.push_str(cfg.element_type);
        }
        IndexShape::Categorized { .. } => s.push_str("id -> {recipe, category}"),
    };
    s.push_str(".\n#\n");
    s.push_str("# Populated LAZILY on first access via the `_id_index` property\n");
    s.push_str("# getter, NOT at `_init`. Godot constructs a Resource (runs\n");
    s.push_str("# `_init`) BEFORE assigning its `@export`ed properties from the\n");
    s.push_str("# `.tres`, so an `_init`-time build would walk an empty Array.\n");
    s.push_str("# The getter defers the build until something actually reads the\n");
    s.push_str("# index, by which point the typed Array(s) are populated. Mod\n");
    s.push_str("# registrations call `_index_add` / `_index_set` / `_index_remove`\n");
    s.push_str("# to keep it in sync afterwards; the typed Array(s) stay canonical.\n");
    s.push_str("var _id_index_storage: Dictionary = {}\n");
    s.push_str("var _id_index_built: bool = false\n");
    s.push_str("var _id_index: Dictionary:\n\tget = _get_id_index\n\n\n");

    s.push_str("func _get_id_index() -> Dictionary:\n");
    s.push_str("\tif not _id_index_built:\n\t\t_rebuild_id_index()\n");
    s.push_str("\treturn _id_index_storage\n\n\n");

    s.push_str("func _rebuild_id_index() -> void:\n");
    s.push_str("\t_id_index_built = true\n");
    s.push_str("\t_id_index_storage.clear()\n");
    match cfg.shape {
        IndexShape::Single { field } => {
            s.push_str(&format!("\tif not ({field} is Array):\n\t\treturn\n"));
            s.push_str(&format!("\tfor r in {field}:\n"));
            s.push_str("\t\tvar id: String = _derive_id(r)\n");
            s.push_str("\t\tif id.is_empty():\n\t\t\tcontinue\n");
            s.push_str("\t\t_id_index_storage[id] = r\n\n\n");
        }
        IndexShape::Categorized { fields } => {
            // Emit a literal Array[String] of the field names so the
            // builder can iterate them generically.
            s.push_str("\tfor cat in [");
            for (i, f) in fields.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                s.push('"');
                s.push_str(f);
                s.push('"');
            }
            s.push_str("]:\n");
            s.push_str("\t\tvar arr: Variant = get(cat)\n");
            s.push_str("\t\tif not (arr is Array):\n\t\t\tcontinue\n");
            s.push_str("\t\tfor r in arr:\n");
            s.push_str("\t\t\tvar id: String = _derive_id(r)\n");
            s.push_str("\t\t\tif id.is_empty():\n\t\t\t\tcontinue\n");
            s.push_str("\t\t\t_id_index_storage[id] = {\"recipe\": r, \"category\": cat}\n\n\n");
        }
    }

    s.push_str("func _derive_id(r: Resource) -> String:\n");
    s.push_str("\tif r == null:\n\t\treturn \"\"\n");
    match cfg.id_source {
        IdSource::ResourcePathStem => {
            s.push_str("\tif r.resource_path.is_empty():\n\t\treturn \"\"\n");
            s.push_str("\treturn r.resource_path.get_file().get_basename()\n\n\n");
        }
        IdSource::Field { name } => {
            // Read the named field; coerce to String. Empty string means
            // the entry lacks an id and gets skipped by the caller.
            s.push_str(&format!("\tvar v: Variant = r.get(\"{name}\")\n"));
            s.push_str("\tif v == null:\n\t\treturn \"\"\n");
            s.push_str("\treturn String(v)\n\n\n");
        }
    }

    s.push_str("# Maintenance helpers used by Lib/shim. Caller is responsible for\n");
    s.push_str("# keeping the typed Array(s) in sync; these only touch the index.\n");
    s.push_str("# Each ensures the base index is built first (via the getter), so a\n");
    s.push_str("# mod registration that lands before any vanilla read isn't wiped\n");
    s.push_str("# by the subsequent lazy rebuild.\n");
    match cfg.shape {
        IndexShape::Single { .. } => {
            let elem = cfg.element_type;
            s.push_str(&format!(
                "func _index_add(id: String, entry: {elem}) -> void:\n\t_get_id_index()[id] = entry\n\n\n"
            ));
            s.push_str(
                "func _index_remove(id: String) -> void:\n\t_get_id_index().erase(id)\n\n\n",
            );
            s.push_str(&format!(
                "func _index_set(id: String, entry: {elem}) -> void:\n\t_get_id_index()[id] = entry\n"
            ));
        }
        IndexShape::Categorized { .. } => {
            let elem = cfg.element_type;
            s.push_str(&format!(
                "func _index_add(id: String, entry: {elem}, category: String) -> void:\n\t_get_id_index()[id] = {{\"recipe\": entry, \"category\": category}}\n\n\n"
            ));
            s.push_str(
                "func _index_remove(id: String) -> void:\n\t_get_id_index().erase(id)\n\n\n",
            );
            s.push_str(&format!(
                "func _index_set(id: String, entry: {elem}, category: String) -> void:\n\t_get_id_index()[id] = {{\"recipe\": entry, \"category\": category}}\n"
            ));
        }
    }

    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_shape_appends_block() {
        let cfg = IdIndexConfig {
            class_name: "Events",
            element_type: "EventData",
            shape: IndexShape::Single { field: "events" },
            id_source: IdSource::ResourcePathStem,
        };
        let src = "extends Resource\nclass_name Events\n@export var events: Array[EventData]\n";
        let out = transform(src, &cfg);
        // Lazy shape: backing storage + built flag + property getter,
        // no `_init`.
        assert!(
            out.contains("var _id_index_storage: Dictionary = {}"),
            "{out}"
        );
        assert!(out.contains("var _id_index_built: bool = false"), "{out}");
        assert!(out.contains("var _id_index: Dictionary:"), "{out}");
        assert!(out.contains("get = _get_id_index"), "{out}");
        assert!(!out.contains("func _init"), "{out}");
        assert!(out.contains("func _get_id_index() -> Dictionary:"), "{out}");
        assert!(out.contains("func _rebuild_id_index() -> void:"), "{out}");
        assert!(out.contains("for r in events:"), "{out}");
        assert!(out.contains("_id_index_storage[id] = r"), "{out}");
        assert!(
            out.contains("func _index_add(id: String, entry: EventData)"),
            "{out}"
        );
        assert!(out.contains("_get_id_index()[id] = entry"), "{out}");
        assert!(out.contains("func _index_remove(id: String)"), "{out}");
    }

    #[test]
    fn categorized_shape_appends_block() {
        let cfg = IdIndexConfig {
            class_name: "Recipes",
            element_type: "RecipeData",
            shape: IndexShape::Categorized {
                fields: &["consumables", "weapons"],
            },
            id_source: IdSource::ResourcePathStem,
        };
        let src = "extends Resource\nclass_name Recipes\n@export var consumables: Array[RecipeData]\n@export var weapons: Array[RecipeData]\n";
        let out = transform(src, &cfg);
        assert!(
            out.contains(r#"for cat in ["consumables", "weapons"]:"#),
            "{out}"
        );
        assert!(out.contains("var arr: Variant = get(cat)"), "{out}");
        assert!(
            out.contains(r#"_id_index_storage[id] = {"recipe": r, "category": cat}"#),
            "{out}"
        );
        assert!(
            out.contains("func _index_add(id: String, entry: RecipeData, category: String)"),
            "{out}"
        );
        assert!(
            out.contains(r#"_get_id_index()[id] = {"recipe": entry, "category": category}"#),
            "{out}"
        );
    }

    #[test]
    fn skips_on_class_name_mismatch() {
        let cfg = IdIndexConfig {
            class_name: "Events",
            element_type: "EventData",
            shape: IndexShape::Single { field: "events" },
            id_source: IdSource::ResourcePathStem,
        };
        let src = "extends Resource\n@export var events: Array[EventData]\n";
        assert_eq!(transform(src, &cfg), src);
    }

    #[test]
    fn skips_on_field_mismatch() {
        let cfg = IdIndexConfig {
            class_name: "Events",
            element_type: "EventData",
            shape: IndexShape::Single { field: "events" },
            id_source: IdSource::ResourcePathStem,
        };
        let src = "extends Resource\nclass_name Events\n@export var stuff: Array[EventData]\n";
        assert_eq!(transform(src, &cfg), src);
    }

    #[test]
    fn skips_when_already_injected() {
        let cfg = IdIndexConfig {
            class_name: "Events",
            element_type: "EventData",
            shape: IndexShape::Single { field: "events" },
            id_source: IdSource::ResourcePathStem,
        };
        let src = "extends Resource\nclass_name Events\n@export var events: Array[EventData]\nvar _id_index: Dictionary = {}\n";
        assert_eq!(transform(src, &cfg), src);
    }

    #[test]
    fn skips_when_existing_init() {
        let cfg = IdIndexConfig {
            class_name: "Events",
            element_type: "EventData",
            shape: IndexShape::Single { field: "events" },
            id_source: IdSource::ResourcePathStem,
        };
        let src = "extends Resource\nclass_name Events\n@export var events: Array[EventData]\nfunc _init():\n\tpass\n";
        assert_eq!(transform(src, &cfg), src);
    }

    #[test]
    fn id_source_field_emits_field_lookup() {
        // Items use ItemData.file as their id, not a file-stem.
        let cfg = IdIndexConfig {
            class_name: "LootTable",
            element_type: "ItemData",
            shape: IndexShape::Single { field: "items" },
            id_source: IdSource::Field { name: "file" },
        };
        let src = "extends Resource\nclass_name LootTable\n@export var items: Array[ItemData]\n";
        let out = transform(src, &cfg);
        assert!(out.contains(r#"var v: Variant = r.get("file")"#), "{out}");
        assert!(out.contains("return String(v)"), "{out}");
        // ResourcePathStem branch should NOT appear.
        assert!(!out.contains("get_basename"), "{out}");
    }

    #[test]
    fn categorized_skips_when_any_field_missing() {
        let cfg = IdIndexConfig {
            class_name: "Recipes",
            element_type: "RecipeData",
            shape: IndexShape::Categorized {
                fields: &["consumables", "weapons", "missing_one"],
            },
            id_source: IdSource::ResourcePathStem,
        };
        let src = "extends Resource\nclass_name Recipes\n@export var consumables: Array[RecipeData]\n@export var weapons: Array[RecipeData]\n";
        assert_eq!(transform(src, &cfg), src);
    }
}
