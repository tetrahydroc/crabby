# ---- scenes kind ----
#
# The rewriter (database_transform.rs) injects three dicts and a _get
# override into Database.gd at bake time:
#   _rtv_vanilla_scenes   - original `const X = preload(...)` lines moved here
#   _rtv_mod_scenes       - register() writes here
#   _rtv_override_scenes  - override() writes here
#
# Database.get(name) routes through the injected _get with precedence
# override > mod > vanilla. Game code doing `Database.Potato` (member
# access on the autoload) hits the same path.

func _database_node() -> Node:
	var db: Node = get_tree().root.get_node_or_null("Database")
	if db == null:
		push_warning("[Registry] Database autoload not in tree yet; is the loader still booting?")
	return db


func _register_scene(id: String, data: Variant) -> bool:
	if not (data is PackedScene):
		push_warning("[Registry] register('scenes', '%s', ...) expects a PackedScene, got %s" % [id, typeof(data)])
		return false
	var db: Node = _database_node()
	if db == null:
		return false
	# The rewriter injects fields only when at least one mod uses scenes
	# (or when always-on per crabby's bake config). If they're missing,
	# writes silently land as ad-hoc properties that vanilla code can't
	# see. Fail loud so mod authors see the real cause.
	if not ("_rtv_mod_scenes" in db):
		push_warning("[Registry] register('scenes', '%s'): Database.gd is missing injected scene fields (rewriter didn't fire)" % id)
		return false
	if _scene_exists_in_vanilla(db, id):
		push_warning("[Registry] register('scenes', '%s'): id collides with vanilla constant; use override instead" % id)
		return false
	if db._rtv_mod_scenes.has(id):
		push_warning("[Registry] register('scenes', '%s'): already registered by a mod" % id)
		return false
	db._rtv_mod_scenes[id] = data
	_track_registered("scenes", id)
	return true


func _override_scene(id: String, data: Variant) -> bool:
	if not (data is PackedScene):
		push_warning("[Registry] override('scenes', '%s', ...) expects a PackedScene, got %s" % [id, typeof(data)])
		return false
	var db: Node = _database_node()
	if db == null:
		return false
	var original: Variant = db.get(id)
	if original == null:
		push_warning("[Registry] override('scenes', '%s'): no existing entry to override" % id)
		return false
	# Reject double-override so a later mod doesn't silently displace an
	# earlier mod's work.
	var ov: Dictionary = _registry_overridden.get("scenes", {})
	if ov.has(id):
		push_warning("[Registry] override('scenes', '%s'): already overridden (revert first to re-override)" % id)
		return false
	ov[id] = original
	_registry_overridden["scenes"] = ov
	db._rtv_override_scenes[id] = data
	return true


func _remove_scene(id: String) -> bool:
	var db: Node = _database_node()
	if db == null:
		return false
	if not db._rtv_mod_scenes.has(id):
		push_warning("[Registry] remove('scenes', '%s'): not registered by a mod" % id)
		return false
	db._rtv_mod_scenes.erase(id)
	var reg: Dictionary = _registry_registered.get("scenes", {})
	reg.erase(id)
	_registry_registered["scenes"] = reg
	return true


func _revert_scene(id: String) -> bool:
	var db: Node = _database_node()
	if db == null:
		return false
	if not db._rtv_override_scenes.has(id):
		push_warning("[Registry] revert('scenes', '%s'): no mod override to revert" % id)
		return false
	db._rtv_override_scenes.erase(id)
	var ov: Dictionary = _registry_overridden.get("scenes", {})
	ov.erase(id)
	_registry_overridden["scenes"] = ov
	return true


## A scene id collides with vanilla if Database's rewritten
## `_rtv_vanilla_scenes` dict contains it. The rewriter moves every
## `const X = preload(...)` from vanilla Database.gd into that dict;
## it's the canonical source of truth for vanilla-shipped names.
func _scene_exists_in_vanilla(db: Node, id: String) -> bool:
	if not ("_rtv_vanilla_scenes" in db):
		return false
	var vs: Dictionary = db._rtv_vanilla_scenes as Dictionary
	return vs.has(id)
