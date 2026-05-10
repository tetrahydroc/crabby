//! Unit tests for [`parse_script`].
//!
//! Each test is a hand-crafted edge case. Add a test here before adding a
//! fixture in `tests/` so behavior is pinned at the smallest possible level.

use super::*;

// --- trivial shape -------------------------------------------------------

#[test]
fn extends_and_class_name() {
    let src = "\
extends Node3D
class_name Hitbox
";
    let p = parse_script("Hitbox.gd", src).unwrap();
    assert_eq!(p.filename, "Hitbox.gd");
    assert_eq!(p.path, "res://Scripts/Hitbox.gd");
    assert_eq!(p.extends.as_deref(), Some("Node3D"));
    assert_eq!(p.class_name.as_deref(), Some("Hitbox"));
    assert!(p.functions.is_empty());
    assert!(p.var_names.is_empty());
}

#[test]
fn extends_with_path_literal() {
    let src = r#"extends "res://Scripts/Camera.gd"
"#;
    let p = parse_script("Sub.gd", src).unwrap();
    // Vostok's regex captures the quoted form too; our regex does the same.
    assert!(
        p.extends
            .as_deref()
            .is_some_and(|s| s.contains("Camera.gd"))
    );
}

// --- top-level vars ------------------------------------------------------

#[test]
fn top_level_vars_captured() {
    let src = "\
extends Node

var health = 100
@export var ammo: int = 30
var _private_flag := false
";
    let p = parse_script("X.gd", src).unwrap();
    assert_eq!(p.var_names, vec!["health", "ammo", "_private_flag"]);
}

#[test]
fn indented_vars_excluded() {
    // Indented `var` is a local inside a function body; must not appear
    // in script-level var_names.
    let src = "\
extends Node

func foo():
\tvar local_a := 1
\tvar local_b := 2

var top_level := 3
";
    let p = parse_script("X.gd", src).unwrap();
    assert_eq!(p.var_names, vec!["top_level"]);
}

// --- function detection --------------------------------------------------

#[test]
fn basic_func_with_typed_params_and_return() {
    let src = "\
extends Node
func add(a: int, b: int) -> int:
\treturn a + b
";
    let p = parse_script("X.gd", src).unwrap();
    assert_eq!(p.functions.len(), 1);
    let f = &p.functions[0];
    assert_eq!(f.name, "add");
    assert_eq!(f.params, "a: int, b: int");
    assert_eq!(f.param_names, vec!["a", "b"]);
    assert_eq!(f.return_type.as_deref(), Some("int"));
    assert!(f.has_return_value);
    assert!(!f.is_static);
    assert!(!f.is_coroutine);
    assert_eq!(f.line_number, 2);
}

#[test]
fn static_func_flag_set() {
    let src = "\
extends Node
static func util() -> String:
\treturn \"ok\"
";
    let p = parse_script("X.gd", src).unwrap();
    let f = &p.functions[0];
    assert_eq!(f.name, "util");
    assert!(f.is_static);
    assert_eq!(f.return_type.as_deref(), Some("String"));
}

#[test]
fn coroutine_flag_from_await() {
    let src = "\
extends Node
func wait_a_frame():
\tawait get_tree().process_frame
\treturn
";
    let p = parse_script("X.gd", src).unwrap();
    let f = &p.functions[0];
    assert!(f.is_coroutine);
    assert!(!f.has_return_value);
}

#[test]
fn explicit_void_overrides_body_returns() {
    // Body has `return <expr>` but the signature says `-> void`; explicit
    // annotation wins (vostok precedent).
    let src = "\
extends Node
func noisy() -> void:
\tprint(\"side effect\")
\treturn # bare
";
    let p = parse_script("X.gd", src).unwrap();
    let f = &p.functions[0];
    assert_eq!(f.return_type.as_deref(), Some("void"));
    assert!(!f.has_return_value);
}

#[test]
fn return_value_inferred_without_annotation() {
    let src = "\
extends Node
func compute():
\tvar x = 1
\treturn x
";
    let p = parse_script("X.gd", src).unwrap();
    let f = &p.functions[0];
    assert!(f.return_type.is_none());
    assert!(f.has_return_value);
}

#[test]
fn typed_collection_return_captured() {
    let src = "\
extends Node
func list_ints() -> Array[int]:
\treturn [1, 2]
";
    let p = parse_script("X.gd", src).unwrap();
    let f = &p.functions[0];
    assert_eq!(f.return_type.as_deref(), Some("Array[int]"));
    assert!(f.has_return_value);
}

// --- order + line numbering ----------------------------------------------

#[test]
fn multiple_functions_line_numbers_are_one_based() {
    let src = "\
extends Node

func a():
\tpass

func b():
\tpass
";
    let p = parse_script("X.gd", src).unwrap();
    assert_eq!(p.functions.len(), 2);
    assert_eq!(p.functions[0].name, "a");
    assert_eq!(p.functions[0].line_number, 3);
    assert_eq!(p.functions[1].name, "b");
    assert_eq!(p.functions[1].line_number, 6);
}

// --- typed declarations ---------------------------------------------------

#[test]
fn typed_decls_module_export_var() {
    let src = "\
extends Node

@export var slotData: SlotData
@export var name: String

func ignore():
\tpass
";
    let p = parse_script("X.gd", src).unwrap();
    let module: Vec<_> = p
        .typed_decls
        .iter()
        .filter(|d| d.scope == DeclScope::Module)
        .collect();
    assert_eq!(module.len(), 2);
    assert_eq!(module[0].name, "slotData");
    assert_eq!(module[0].type_name, "SlotData");
    assert_eq!(module[1].name, "name");
    assert_eq!(module[1].type_name, "String");
}

#[test]
fn typed_decls_function_params() {
    let src = "\
extends Node

func Update(slotData: SlotData, amount: int):
\tpass
";
    let p = parse_script("X.gd", src).unwrap();
    let func_line = p.functions[0].line_number;
    let scoped: Vec<_> = p
        .typed_decls
        .iter()
        .filter(|d| matches!(d.scope, DeclScope::Function { func_line: fl } if fl == func_line))
        .collect();
    assert_eq!(scoped.len(), 2);
    assert_eq!(scoped[0].name, "slotData");
    assert_eq!(scoped[0].type_name, "SlotData");
    assert_eq!(scoped[1].name, "amount");
    assert_eq!(scoped[1].type_name, "int");
}

#[test]
fn typed_decls_local_var_explicit() {
    let src = "\
extends Node

func DoIt():
\tvar slot: SlotData = make()
\tslot.do_thing()
";
    let p = parse_script("X.gd", src).unwrap();
    let func_line = p.functions[0].line_number;
    let local: Vec<_> = p
        .typed_decls
        .iter()
        .filter(|d| matches!(d.scope, DeclScope::Function { func_line: fl } if fl == func_line))
        .collect();
    assert_eq!(local.len(), 1);
    assert_eq!(local[0].name, "slot");
    assert_eq!(local[0].type_name, "SlotData");
}

#[test]
fn typed_decls_local_var_inferred_new() {
    let src = "\
extends Node

func DoIt():
\tvar fresh := SlotData.new()
\tvar also = ItemSave.new()
";
    let p = parse_script("X.gd", src).unwrap();
    let func_line = p.functions[0].line_number;
    let names: Vec<(&str, &str)> = p
        .typed_decls
        .iter()
        .filter(|d| matches!(d.scope, DeclScope::Function { func_line: fl } if fl == func_line))
        .map(|d| (d.name.as_str(), d.type_name.as_str()))
        .collect();
    assert_eq!(names, vec![("fresh", "SlotData"), ("also", "ItemSave")]);
}

#[test]
fn typed_decls_scope_per_function() {
    let src = "\
extends Node

func A(x: SlotData):
\tpass

func B(y: ItemData):
\tpass
";
    let p = parse_script("X.gd", src).unwrap();
    let f1 = p.functions[0].line_number;
    let f2 = p.functions[1].line_number;

    let in_a: Vec<_> = p
        .typed_decls
        .iter()
        .filter(|d| matches!(d.scope, DeclScope::Function { func_line: fl } if fl == f1))
        .collect();
    let in_b: Vec<_> = p
        .typed_decls
        .iter()
        .filter(|d| matches!(d.scope, DeclScope::Function { func_line: fl } if fl == f2))
        .collect();

    assert_eq!(in_a.len(), 1);
    assert_eq!(in_a[0].name, "x");
    assert_eq!(in_a[0].type_name, "SlotData");
    assert_eq!(in_b.len(), 1);
    assert_eq!(in_b[0].name, "y");
    assert_eq!(in_b[0].type_name, "ItemData");
}

#[test]
fn untyped_decls_are_not_recorded() {
    let src = "\
extends Node

@export var anything

func DoIt(p, q := 5):
\tvar x = make()
\tvar y := compute()
";
    let p = parse_script("X.gd", src).unwrap();
    // None of these have an explicit type annotation; the inferred-new
    // pattern requires `Type.new(`, not arbitrary calls. Expect zero
    // typed decls.
    assert!(p.typed_decls.is_empty(), "got {:?}", p.typed_decls);
}
