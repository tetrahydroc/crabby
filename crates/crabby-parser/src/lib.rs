//! `GDScript` source → typed declaration structures.
//!
//! Extracts the shape the rewriter needs: `extends`, `class_name`, top-level
//! `var` names, and per-function signatures (name, params, return type,
//! static-ness, await-usage). Not a full AST; just enough structure to drive
//! per-method wrapper-template selection and injection.
//!
//! Port of vostok-mod-loader's `_rtv_parse_script` in `src/rewriter.gd`.
//!
//! # Output
//!
//! ```
//! use crabby_parser::parse_script;
//!
//! let src = "\
//! extends Node
//! class_name Hitbox
//!
//! @export var type: String
//!
//! func ApplyDamage(damage: float) -> void:
//!     return
//! ";
//! let parsed = parse_script("Hitbox.gd", src).unwrap();
//!
//! assert_eq!(parsed.extends.as_deref(), Some("Node"));
//! assert_eq!(parsed.class_name.as_deref(), Some("Hitbox"));
//! assert_eq!(parsed.var_names, vec!["type"]);
//! assert_eq!(parsed.functions.len(), 1);
//! assert_eq!(parsed.functions[0].name, "ApplyDamage");
//! assert_eq!(parsed.functions[0].param_names, vec!["damage"]);
//! assert_eq!(parsed.functions[0].return_type.as_deref(), Some("void"));
//! assert!(!parsed.functions[0].has_return_value);
//! ```
//!
//! # Error convention
//!
//! Leaf failures convert into [`CrabbyError::Parse`]:
//!
//! ```
//! use crabby_error::{CrabbyError, Result};
//!
//! fn require_extends(source: &str) -> Result<&str> {
//!     source
//!         .lines()
//!         .find_map(|l| l.strip_prefix("extends "))
//!         .ok_or_else(|| CrabbyError::Parse {
//!             context: "no `extends` clause".into(),
//!             source: "scripts must extend a base type".into(),
//!         })
//! }
//!
//! assert!(require_extends("class_name Foo\n").is_err());
//! assert_eq!(require_extends("extends Node\n").unwrap(), "Node");
//! ```
//!
//! [`CrabbyError::Parse`]: crabby_error::CrabbyError::Parse

#![deny(missing_docs)]

mod function;
mod params;
mod parser;
mod regex_set;
mod typed_decls;

pub use function::FuncDecl;
pub use parser::{ParsedScript, parse_script};
pub use typed_decls::{DeclScope, TypedDecl};
