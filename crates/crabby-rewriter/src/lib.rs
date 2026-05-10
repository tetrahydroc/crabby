//! Parsed script + wrapper template -> rewritten `GDScript` source.
//!
//! Emits per-method dispatch wrappers (and per-script injections for
//! data-intercept targets) that route through the runtime shim. Picks
//! from five templates based on parse-time analysis, see
//! [`TemplateKind`] for the per-method set.
//!
//! - `void` / `non_void`, ordinary scripts.
//! - `fast`, per-frame / per-shot / short-lived scripts.
//! - `additive`, save-serialized `Resource` scripts.
//! - `data_intercept`, pure-data `Resource` scripts (script-level
//!   injection; no per-method wrapping).
//!
//! Coroutine handling is template-intrinsic: when `FuncDecl::is_coroutine`
//! is true, every template prefixes `await ` at every vanilla-call site.
//! No separate `await_aware` template exists.
//!
//! # Error convention
//!
//! Leaf failures convert into [`CrabbyError::Rewrite`]:
//!
//! ```
//! use crabby_error::{CrabbyError, Result};
//!
//! fn require_template(name: &str) -> Result<&'static str> {
//!     match name {
//!         "standard" | "fast" | "await_aware" | "additive" | "data_intercept" => Ok("ok"),
//!         other => Err(CrabbyError::Rewrite {
//!             context: format!("unknown template {other:?}"),
//!             source: "template selection out of range".into(),
//!         }),
//!     }
//! }
//!
//! assert!(require_template("standard").is_ok());
//! assert!(require_template("laser").is_err());
//! ```
//!
//! [`CrabbyError::Rewrite`]: crabby_error::CrabbyError::Rewrite

#![deny(missing_docs)]

mod ai_select_weapon_transform;
mod ai_spawner_transform;
mod autofix;
mod call_rewriter;
mod compiler_spawn_transform;
mod data_intercept;
mod database_transform;
mod engine_void;
mod events_index_transform;
mod fish_pool_transform;
mod full_script;
mod hook_name;
mod id_index_transform;
mod loader_transform;
mod loot_table_index_transform;
mod normalize;
mod recipes_index_transform;
mod renamer;
mod resource_serialized;
mod rewrite;
mod runtime_incompatible;
mod template;
mod template_selection;
mod trader_data_index_transform;

pub use ai_select_weapon_transform::{AI_FILENAME, AI_LOADOUTS_ENGINE_META_KEY};
pub use ai_spawner_transform::{AI_ENGINE_META_KEY, AI_SPAWNER_FILENAME};
pub use autofix::inject_pass_into_bodyless_blocks;
pub use call_rewriter::rewrite_consumer_calls;
pub use compiler_spawn_transform::COMPILER_FILENAME;
pub use data_intercept::{
    DATA_INTERCEPT_SCRIPTS, PATCH_DICT_VAR_NAME, is_data_intercept_script,
    should_inject_data_intercept,
};
pub use database_transform::{
    DATABASE_FILENAME, MOD_SCENES_VAR, OVERRIDE_SCENES_VAR, VANILLA_SCENES_VAR,
    emit_registry_injection, is_database_script, rewrite_database_constants,
};
pub use engine_void::is_engine_void_method;
pub use events_index_transform::EVENTS_SCHEMA_FILENAME;
pub use fish_pool_transform::{FISH_ENGINE_META_KEY, FISH_POOL_FILENAME};
pub use full_script::{rewrite_full_script, rewrite_full_script_with_hooks};
pub use hook_name::{HookFlags, hook_base, script_prefix};
pub use loader_transform::{
    LOADER_FILENAME, MOD_SCENE_PATHS_VAR, MOD_SHELTERS_VAR, OVERRIDE_SCENE_PATHS_VAR,
    VANILLA_SHELTERS_VAR,
};
pub use loot_table_index_transform::LOOT_TABLE_SCHEMA_FILENAME;
pub use recipes_index_transform::RECIPES_SCHEMA_FILENAME;
pub use resource_serialized::{
    ADDITIVE_HOOK_PREFIX, ADDITIVE_TEMPLATE_SCRIPTS, is_additive_script,
};
pub use rewrite::rewrite_single_method;
pub use runtime_incompatible::{RUNTIME_INCOMPATIBLE_SCRIPTS, is_runtime_incompatible};
pub use template_selection::{FAST_TEMPLATE_SCRIPTS, TemplateKind, is_fast_script, pick_template};
pub use trader_data_index_transform::TRADER_DATA_SCHEMA_FILENAME;
