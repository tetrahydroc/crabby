# ---- items kind ----

const _LT_MASTER_PATH := "res://Loot/LT_Master.tres"


## Cached load() of LT_Master.tres. Same instance every time (Godot's
## resource cache); _id_index lives on it via the bake-time injection
## (see crabby-rewriter::loot_table_index_transform).
var _master_cache: Resource = null


func _master_resource() -> Resource:
	if _master_cache != null:
		return _master_cache
	_master_cache = load(_LT_MASTER_PATH)
	return _master_cache


func _register_item(id: String, data: Variant) -> bool:
	if not (data is Resource):
		push_warning("[Registry] register('items', '%s'): expected a Resource (ItemData), got %s" % [id, typeof(data)])
		return false
	if _find_vanilla_item(id) != null:
		push_warning("[Registry] register('items', '%s'): id collides with vanilla item; use override or patch" % id)
		return false
	if _mod_added_has("items", id):
		push_warning("[Registry] register('items', '%s'): already registered by a mod" % id)
		return false
	# Keep ItemData.file in sync with the registry id; vanilla code reads
	# itemData.file directly and assumes it matches the canonical name.
	if data.get("file") != id:
		data.set("file", id)
	# Drive the vanilla-side _id_index injected onto LootTable.gd.
	# Deliberately does NOT append to master.items here; vanilla item
	# enumeration via `for it in Database.master.items` is loot-table-
	# scoped semantics; mod items get into specific tables via the loot
	# registry, not by joining the master catalog wholesale.
	var master: Resource = _master_resource()
	if master != null and master.has_method("_index_add"):
		master._index_add(id, data)
	_mod_added_mark("items", id)
	return true


func _override_item(id: String, data: Variant) -> bool:
	if not (data is Resource):
		push_warning("[Registry] override('items', '%s'): expected a Resource (ItemData), got %s" % [id, typeof(data)])
		return false
	var existing: Resource = _lookup_item(id)
	if existing == null:
		push_warning("[Registry] override('items', '%s'): no existing item to override" % id)
		return false
	var ov: Dictionary = _registry_overridden.get("items", {})
	if not ov.has(id):
		ov[id] = existing
		_registry_overridden["items"] = ov
	if data.get("file") != id:
		data.set("file", id)
	# Replace the slot in the vanilla-side _id_index with the new ItemData.
	var master: Resource = _master_resource()
	if master != null and master.has_method("_index_set"):
		master._index_set(id, data)
	_mod_added_mark("items", id)
	return true


func _patch_item(id: String, fields: Dictionary) -> bool:
	if fields.is_empty():
		push_warning("[Registry] patch('items', '%s'): empty fields dict is a no-op" % id)
		return false
	var target: Resource = _lookup_item(id)
	if target == null:
		push_warning("[Registry] patch('items', '%s'): no item with that id" % id)
		return false
	var patched: Dictionary = _registry_patched.get("items", {})
	var stash: Dictionary = patched.get(id, {})
	for field in fields.keys():
		var field_name := String(field)
		if not _resource_has_property(target, field_name):
			push_warning("[Registry] patch('items', '%s'): field '%s' doesn't exist on %s" \
					% [id, field_name, target.get_class()])
			continue
		# First-write-wins stash: keep the pre-any-patch value so revert
		# restores true vanilla regardless of how many patches piled on.
		if not stash.has(field_name):
			stash[field_name] = target.get(field_name)
		target.set(field_name, fields[field])
	patched[id] = stash
	_registry_patched["items"] = patched
	return true


func _remove_item(id: String) -> bool:
	if not _mod_added_has("items", id):
		push_warning("[Registry] remove('items', '%s'): not registered by a mod" % id)
		return false
	var ov: Dictionary = _registry_overridden.get("items", {})
	if ov.has(id):
		push_warning("[Registry] remove('items', '%s'): entry is an override, use revert instead" % id)
		return false
	# Drop from vanilla-side index.
	var master: Resource = _master_resource()
	if master != null and master.has_method("_index_remove"):
		master._index_remove(id)
	_mod_added_unmark("items", id)
	return true


func _revert_item(id: String, fields: Array) -> bool:
	var did_something := false
	var ov: Dictionary = _registry_overridden.get("items", {})
	var patched: Dictionary = _registry_patched.get("items", {})
	# Full revert: no fields specified -> undo override AND clear patches.
	# Order matters: restore patches first (onto the currently-resolving
	# entry, which may be the override), then drop the override so lookups
	# fall back to vanilla. Reversing would write patch values onto vanilla,
	# permanently mutating the base resource.
	if fields.is_empty():
		if patched.has(id):
			var target: Resource = _lookup_item(id)
			if target != null:
				var stash: Dictionary = patched[id]
				for fname in stash.keys():
					target.set(fname, stash[fname])
			patched.erase(id)
			_registry_patched["items"] = patched
			did_something = true
		if ov.has(id):
			var reg: Dictionary = _registry_registered.get("items", {})
			reg.erase(id)
			_registry_registered["items"] = reg
			ov.erase(id)
			_registry_overridden["items"] = ov
			did_something = true
		if not did_something:
			push_warning("[Registry] revert('items', '%s'): nothing to revert" % id)
		return did_something
	# Per-field revert: only undo the named fields on a patch.
	if not patched.has(id):
		push_warning("[Registry] revert('items', '%s', %s): no patches on this id" % [id, fields])
		return false
	var target: Resource = _lookup_item(id)
	if target == null:
		push_warning("[Registry] revert('items', '%s', %s): id no longer resolves" % [id, fields])
		return false
	var stash: Dictionary = patched[id]
	for field in fields:
		var fname := String(field)
		if not stash.has(fname):
			push_warning("[Registry] revert('items', '%s'): field '%s' wasn't patched" % [id, fname])
			continue
		target.set(fname, stash[fname])
		stash.erase(fname)
		did_something = true
	if stash.is_empty():
		patched.erase(id)
	else:
		patched[id] = stash
	_registry_patched["items"] = patched
	return did_something


## Lookup precedence: mod registrations (which include overrides) > vanilla.
func _lookup_item(id: String) -> Resource:
	# Vanilla `LT_Master._id_index` is the single source of truth; it
	# holds vanilla items keyed by ItemData.file AND mod-added items
	# keyed by the mod-supplied id (also stored as itemData.file). Mod
	# overrides on vanilla ids replace the entry under the same id-key
	# (see _override_item).
	return _find_vanilla_item(id)


func _find_vanilla_item(id: String) -> Resource:
	var master: Resource = _master_resource()
	if master == null or not ("_id_index" in master):
		return null
	return master._id_index.get(id)


## Resource.get() returns null both for "missing property" and "legitimate


# ---- array-ops on item Resource fields (compatible, fits, etc.) ----

func _append_item(id: String, field: String, values: Array, allow_duplicates: bool) -> bool:
	var target: Resource = _lookup_item(id)
	if target == null:
		push_warning("[Registry] append('items', '%s'): no item with that id" % id)
		return false
	return _array_op_on_resource("items", id, target, field, "append", values, allow_duplicates)


func _prepend_item(id: String, field: String, values: Array, allow_duplicates: bool) -> bool:
	var target: Resource = _lookup_item(id)
	if target == null:
		push_warning("[Registry] prepend('items', '%s'): no item with that id" % id)
		return false
	return _array_op_on_resource("items", id, target, field, "prepend", values, allow_duplicates)


func _remove_from_item(id: String, field: String, values: Array) -> bool:
	var target: Resource = _lookup_item(id)
	if target == null:
		push_warning("[Registry] remove_from('items', '%s'): no item with that id" % id)
		return false
	return _array_op_on_resource("items", id, target, field, "remove_from", values, false)
