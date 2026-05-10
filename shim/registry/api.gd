# ============================================================================
# Registry - vostok-parity surface
# ============================================================================
#
# Mods use `lib.Registry.<KIND>` constants and the seven verbs
# `register / override / patch / remove / revert / has_entry / get_entry`
# to mutate the game's content (items, scenes, loot, etc.). Per-kind
# semantics live in the `_register_<kind>` helpers.
#
# The registry mutates **live Resource references directly** via
# `target.set(field, value)`. PCK rewrite owns the script class binding
# for ItemData (and all other Resource subclasses), so the live
# `LT_Master.tres`-loaded ItemData instances are bound to that class
# and `set()` writes the actual storage slot. Every downstream consumer
# that reads `item.value` sees the new value.


## Registry kind constants. Mods write `lib.Registry.ITEMS` instead of
## the raw string `"items"` so typos surface at parse time.
const Registry := {
	SCENES = "scenes",
	ITEMS = "items",
	LOOT = "loot",
	SOUNDS = "sounds",
	RECIPES = "recipes",
	EVENTS = "events",
	TRADER_POOLS = "trader_pools",
	TRADER_TASKS = "trader_tasks",
	INPUTS = "inputs",
	SCENE_PATHS = "scene_paths",
	SHELTERS = "shelters",
	MAPS = "maps",
	RANDOM_SCENES = "random_scenes",
	AI_TYPES = "ai_types",
	AI_LOADOUTS = "ai_loadouts",
	FISH_SPECIES = "fish_species",
	RESOURCES = "resources",
	SCENE_NODES = "scene_nodes",
	WEAPONS = "weapons",
	MAGAZINES = "magazines",
	ATTACHMENTS = "attachments",
}

## kind -> {id -> data} for entries created via register(). For kinds
## where vanilla owns an `_id_index` on the Resource (recipes, events,
## items, trader_tasks, loot), this stays only as a "is-this-mod-added?"
## marker (see `_mod_added_ids` below) and the per-entry value is the
## same data already held in the vanilla index. For kinds without a
## vanilla index (loot, trader_pools, random_scenes, ai_*, fish_species,
## inputs, shelters, sounds), this dict IS the data store.
var _registry_registered: Dictionary = {}
## kind -> {id -> true} for entries created via register() on kinds that
## have a vanilla `_id_index` on their Resource. The id-index holds the
## actual data; this Set just answers "did a mod register this id?" for
## the `include_vanilla=false` filter and the vanilla-enumeration de-dup
## merge. Populated/dropped alongside register/remove on the migrated
## kinds.
var _mod_added_ids: Dictionary = {}
## kind -> {id -> original_data} for entries replaced via override().
var _registry_overridden: Dictionary = {}
## kind -> {id -> {field_name -> original_value}} for fields mutated via patch().
## First-write-wins: the stash is the value as it was BEFORE any patch on that
## field, so revert restores true vanilla regardless of patch count.
var _registry_patched: Dictionary = {}


## Mark `id` as mod-added under `kind` in the lightweight Set. Used by
## migrated kinds where vanilla `_id_index` holds the data and only a
## presence flag is needed here.
func _mod_added_mark(kind: String, id: String) -> void:
	var s: Dictionary = _mod_added_ids.get(kind, {})
	s[id] = true
	_mod_added_ids[kind] = s


func _mod_added_unmark(kind: String, id: String) -> void:
	var s: Dictionary = _mod_added_ids.get(kind, {})
	s.erase(id)
	_mod_added_ids[kind] = s


func _mod_added_has(kind: String, id: String) -> bool:
	var s: Dictionary = _mod_added_ids.get(kind, {})
	return s.has(id)


func _mod_added_set(kind: String) -> Dictionary:
	return _mod_added_ids.get(kind, {})

# ---- Aggregator helpers (one-shot bundle registrations) ----
#
# Each helper wraps several primitive registries (ITEMS + SCENES + LOOT
# + TRADER_POOLS, plus patches to vanilla weapons' `compatible`) into a
# single call and returns a Dictionary with per-step success bools. The
# Registry-const verbs `register('weapons', ...)` / `register('magazines',
# ...)` / `register('attachments', ...)` collapse the dict to a single
# bool; call these helpers directly for the granular result.
#
# `register_item` is method-only (no Registry const) since the
# bare-Resource form of `register('items', ...)` already covers it.

## Register one or more generic ItemData bundles. Always takes a Dictionary
## of {id: data}, even for a single registration. Returns
## {ok, results: {id: granular_dict}}.
func register_item(entries: Dictionary) -> Dictionary:
	return _register_aggregator_batch("item", entries)


## Register one or more furniture bundles. Same shape as register_item.
func register_furniture(entries: Dictionary) -> Dictionary:
	return _register_aggregator_batch("furniture", entries)


## Register one or more weapon bundles. Same shape as register_item.
func register_weapon(entries: Dictionary) -> Dictionary:
	return _register_aggregator_batch("weapon", entries)


## Register one or more magazine bundles. Same shape as register_item.
func register_magazine(entries: Dictionary) -> Dictionary:
	return _register_aggregator_batch("magazine", entries)


## Register one or more attachment bundles. Same shape as register_item.
func register_attachment(entries: Dictionary) -> Dictionary:
	return _register_aggregator_batch("attachment", entries)


## Batch-register entries against the ai_loadouts primitive registry.
## Per-entry data: {weapon_scene, ai_types[], chance?, replace?}. See
## registry/ai_loadouts.gd for shape detail. Standalone helper for
## loadouts the mod doesn't otherwise own; weapons registered via
## register_weapon can declare `ai_loadout` inline as a shortcut.
func register_ai_loadout(entries: Dictionary) -> Dictionary:
	return _register_aggregator_batch("ai_loadout", entries)


# Shared loop for all aggregator helpers. Mirrors the main loader's
# _register_aggregator_batch; keeps the crabby shim API-compatible.
func _register_aggregator_batch(kind: String, entries: Dictionary) -> Dictionary:
	var results: Dictionary = {}
	var all_ok := true
	for id in entries.keys():
		var sid := String(id)
		var per: Dictionary
		match kind:
			"item":       per = _register_item_bundle(sid, entries[id])
			"furniture":  per = _register_furniture_bundle(sid, entries[id])
			"weapon":     per = _register_weapon(sid, entries[id])
			"magazine":   per = _register_magazine(sid, entries[id])
			"attachment": per = _register_attachment(sid, entries[id])
			"ai_loadout":
				# Thin wrapper: ai_loadouts is a primitive registry, not
				# a multi-step bundle. Wraps the bool result in the same
				# {ok, ...} shape as the other aggregators for batch-loop
				# consistency.
				var ok: bool = _register_ai_loadout(sid, entries[id])
				per = {"ok": ok}
			_:
				per = {"ok": false, "error": "internal: unknown aggregator kind '%s'" % kind}
		results[sid] = per
		if not bool(per.get("ok", false)):
			all_ok = false
	return {"ok": all_ok, "results": results}


## Register a NEW entry under `kind` keyed by `id`. Returns true on success.
## Fails (and logs) if the id already exists in vanilla or a prior mod
## registration. Per-kind semantics live in the `_register_<kind>` helpers.
func register(kind: String, id: String, data: Variant) -> bool:
	if id == "":
		push_warning("[Registry] register(%s, ...) called with empty id" % kind)
		return false
	match kind:
		"items": return _register_item(id, data)
		"loot": return _register_loot(id, data)
		"sounds": return _register_sound(id, data)
		"resources":
			push_warning("[Registry] register: 'resources' doesn't support register (the target .tres already exists in vanilla; use patch to mutate its fields)")
			return false
		"events": return _register_event(id, data)
		"recipes": return _register_recipe(id, data)
		"trader_pools": return _register_trader_pool(id, data)
		"trader_tasks": return _register_trader_task(id, data)
		"inputs": return _register_input(id, data)
		"scenes": return _register_scene(id, data)
		"scene_nodes":
			push_warning("[Registry] register: 'scene_nodes' doesn't support register (nodes are positional inside a scene; use override('scenes', ...) to replace the whole scene or patch('scene_nodes', ...) to mutate node properties)")
			return false
		"scene_paths": return _register_scene_path(id, data)
		"shelters": return _register_shelter(id, data)
		"maps": return _register_map(id, data)
		"random_scenes": return _register_random_scene(id, data)
		"ai_types": return _register_ai_type(id, data)
		"ai_loadouts": return _register_ai_loadout(id, data)
		"fish_species": return _register_fish_species(id, data)
		"weapons": return bool(_register_weapon(id, data).get("ok", false))
		"magazines": return bool(_register_magazine(id, data).get("ok", false))
		"attachments": return bool(_register_attachment(id, data).get("ok", false))
		_:
			push_warning("[Registry] register: '%s' not yet implemented in crabby" % kind)
			return false


## Replace an existing entry. Stashes the original so revert() can restore.
## Fails if the id doesn't currently resolve.
func override(kind: String, id: String, data: Variant) -> bool:
	if id == "":
		push_warning("[Registry] override(%s, ...) called with empty id" % kind)
		return false
	match kind:
		"items": return _override_item(id, data)
		"loot": return _override_loot(id, data)
		"sounds": return _override_sound(id, data)
		"resources":
			push_warning("[Registry] override: 'resources' doesn't support override (vanilla .tres already exists; use patch to mutate fields)")
			return false
		"events": return _override_event(id, data)
		"recipes": return _override_recipe(id, data)
		"trader_pools":
			push_warning("[Registry] override: 'trader_pools' doesn't support override (entries are boolean flags; use register/remove)")
			return false
		"trader_tasks": return _override_trader_task(id, data)
		"inputs": return _override_input(id, data)
		"scenes": return _override_scene(id, data)
		"scene_nodes":
			push_warning("[Registry] override: 'scene_nodes' doesn't support override (whole-scene swap goes through override('scenes', ...); scene_nodes is patch-only)")
			return false
		"scene_paths": return _override_scene_path(id, data)
		"shelters":
			push_warning("[Registry] override: 'shelters' doesn't support override (it's an append-only list; use register/remove)")
			return false
		"maps":
			push_warning("[Registry] override: 'maps' doesn't support override (append-only; use register/remove, or override('scenes', ...) to swap the underlying scene)")
			return false
		"random_scenes":
			push_warning("[Registry] override: 'random_scenes' doesn't support override (append-only list; use register/remove)")
			return false
		"ai_types": return _override_ai_type(id, data)
		"ai_loadouts": return _override_ai_loadout(id, data)
		"fish_species":
			push_warning("[Registry] override: 'fish_species' doesn't support override (entries are {scene, pool_id} refs)")
			return false
		"weapons", "magazines", "attachments":
			push_warning("[Registry] override: '%s' is a pure aggregator -- override the underlying primitives instead (override('items', ...) for the ItemData, override('scenes', ...) for the world/rig scene)" % kind)
			return false
		_:
			push_warning("[Registry] override: '%s' not yet implemented in crabby" % kind)
			return false


## Partial update; merge `fields` into the entry at `id`. First-write-wins
## stash records the pre-any-patch value so revert restores true vanilla.
## Per-kind dispatch; some kinds reject patch (scenes, loot, etc).
func patch(kind: String, id: Variant, fields: Dictionary) -> bool:
	if id is String and id == "":
		push_warning("[Registry] patch(%s, ...) called with empty id" % kind)
		return false
	match kind:
		"items":
			if not (id is String):
				push_warning("[Registry] patch('items', ...): id must be a String")
				return false
			return _patch_item(id, fields)
		"loot":
			push_warning("[Registry] patch: 'loot' doesn't support patch (loot entries are ItemData references; patch the ItemData via the 'items' registry instead)")
			return false
		"sounds":
			if not (id is String):
				push_warning("[Registry] patch('sounds', ...): id must be a String")
				return false
			return _patch_sound(id, fields)
		"resources":
			if not (id is String):
				push_warning("[Registry] patch('resources', ...): id must be a res:// path String")
				return false
			return _patch_resource(id, fields)
		"events": return _patch_event(id, fields)
		"recipes": return _patch_recipe(id, fields)
		"trader_pools":
			push_warning("[Registry] patch: 'trader_pools' doesn't support patch (entries are boolean flags; use register/remove)")
			return false
		"trader_tasks": return _patch_trader_task(id, fields)
		"inputs":
			if not (id is String):
				push_warning("[Registry] patch('inputs', ...): id must be a String")
				return false
			return _patch_input(id, fields)
		"scenes":
			push_warning("[Registry] patch: 'scenes' doesn't support patch (scenes are monolithic PackedScenes; use override instead)")
			return false
		"scene_nodes":
			if not (id is String):
				push_warning("[Registry] patch('scene_nodes', ...): id must be a String '<scene_path>#<node_path>'")
				return false
			return _patch_scene_node(id, fields)
		"scene_paths":
			if not (id is String):
				push_warning("[Registry] patch('scene_paths', ...): id must be a String")
				return false
			return _patch_scene_path(id, fields)
		"shelters":
			push_warning("[Registry] patch: 'shelters' doesn't support patch (use remove + register to change fields, or patch('scene_paths', ...) for path-only edits)")
			return false
		"maps":
			push_warning("[Registry] patch: 'maps' doesn't support patch (use remove + register to change fields, or patch('scene_paths', ...) for path-only edits)")
			return false
		"random_scenes":
			push_warning("[Registry] patch: 'random_scenes' doesn't support patch (entries are bare paths)")
			return false
		"ai_types":
			push_warning("[Registry] patch: 'ai_types' doesn't support patch (entries are {scene, zone} refs; use override to swap the scene)")
			return false
		"ai_loadouts":
			push_warning("[Registry] patch: 'ai_loadouts' doesn't support patch (entries are flat dicts; use override to replace the whole entry)")
			return false
		"fish_species":
			push_warning("[Registry] patch: 'fish_species' doesn't support patch (entries are {scene, pool_id} refs)")
			return false
		"weapons", "magazines", "attachments":
			push_warning("[Registry] patch: '%s' is a pure aggregator -- patch the underlying primitive instead (patch('items', ...) for ItemData fields like compatible/damage/etc)" % kind)
			return false
		_:
			push_warning("[Registry] patch: '%s' not yet implemented in crabby" % kind)
			return false


## Undo a register(). Fails if `id` wasn't registered by a mod.
func remove(kind: String, id: String) -> bool:
	match kind:
		"items": return _remove_item(id)
		"loot": return _remove_loot(id)
		"sounds": return _remove_sound(id)
		"resources":
			push_warning("[Registry] remove: 'resources' doesn't support remove (use revert to undo patches)")
			return false
		"events": return _remove_event(id)
		"recipes": return _remove_recipe(id)
		"trader_pools": return _remove_trader_pool(id)
		"trader_tasks": return _remove_trader_task(id)
		"inputs": return _remove_input(id)
		"scenes": return _remove_scene(id)
		"scene_nodes":
			push_warning("[Registry] remove: 'scene_nodes' doesn't support remove (use revert to undo a property patch)")
			return false
		"scene_paths": return _remove_scene_path(id)
		"shelters": return _remove_shelter(id)
		"maps": return _remove_map(id)
		"random_scenes": return _remove_random_scene(id)
		"ai_types": return _remove_ai_type(id)
		"ai_loadouts": return _remove_ai_loadout(id)
		"fish_species": return _remove_fish_species(id)
		"weapons", "magazines", "attachments":
			push_warning("[Registry] remove: '%s' is a pure aggregator -- remove the underlying primitives instead (remove('items', ...), remove('scenes', ...), remove('loot', ...))" % kind)
			return false
		_:
			push_warning("[Registry] remove: '%s' not yet implemented in crabby" % kind)
			return false


## Undo an override() and/or patch()es. With `fields` empty: undo override
## AND clear all patches on `id`. With `fields` non-empty: per-field patch
## revert only. Returns true if anything actually changed.
func revert(kind: String, id: Variant, fields: Array = []) -> bool:
	match kind:
		"items":
			if not (id is String):
				push_warning("[Registry] revert('items', ...): id must be a String")
				return false
			return _revert_item(id, fields)
		"loot":
			if not (id is String):
				push_warning("[Registry] revert('loot', ...): id must be a String")
				return false
			return _revert_loot(id)
		"sounds":
			if not (id is String):
				push_warning("[Registry] revert('sounds', ...): id must be a String")
				return false
			return _revert_sound(id, fields)
		"resources":
			if not (id is String):
				push_warning("[Registry] revert('resources', ...): id must be a res:// path String")
				return false
			return _revert_resource(id, fields)
		"events": return _revert_event(id, fields)
		"recipes": return _revert_recipe(id, fields)
		"trader_pools":
			if not (id is String):
				push_warning("[Registry] revert('trader_pools', ...): id must be a String")
				return false
			return _revert_trader_pool(id)
		"trader_tasks": return _revert_trader_task(id, fields)
		"inputs":
			if not (id is String):
				push_warning("[Registry] revert('inputs', ...): id must be a String")
				return false
			return _revert_input(id, fields)
		"scenes":
			if not (id is String):
				push_warning("[Registry] revert('scenes', ...): id must be a String")
				return false
			return _revert_scene(id)
		"scene_nodes":
			if not (id is String):
				push_warning("[Registry] revert('scene_nodes', ...): id must be a String '<scene_path>#<node_path>'")
				return false
			return _revert_scene_node(id, fields)
		"scene_paths":
			if not (id is String):
				push_warning("[Registry] revert('scene_paths', ...): id must be a String")
				return false
			return _revert_scene_path(id, fields)
		"shelters":
			# No override layer; revert is an alias for remove.
			if not (id is String):
				push_warning("[Registry] revert('shelters', ...): id must be a String")
				return false
			return _remove_shelter(id)
		"maps":
			if not (id is String):
				push_warning("[Registry] revert('maps', ...): id must be a String")
				return false
			return _remove_map(id)
		"ai_types":
			if not (id is String):
				push_warning("[Registry] revert('ai_types', ...): id must be a String")
				return false
			return _revert_ai_type(id)
		"ai_loadouts":
			if not (id is String):
				push_warning("[Registry] revert('ai_loadouts', ...): id must be a String")
				return false
			return _revert_ai_loadout(id)
		"fish_species":
			# fish_species has no override layer; revert is an alias for remove.
			if not (id is String):
				push_warning("[Registry] revert('fish_species', ...): id must be a String")
				return false
			return _remove_fish_species(id)
		_:
			push_warning("[Registry] revert: '%s' not yet implemented in crabby" % kind)
			return false

## Append values to an Array field on a registry entry. Array-only.
## De-duplicates by default (matches typical mod intent for compatibility
## lists); pass `allow_duplicates = true` to permit repeats. `values`
## accepts a single value or an Array. First-write-wins stash is shared
## with patch(), so revert() restores the true pre-any-mutation array
## even after multiple ops.
func append(kind: String, id: Variant, field: String, values: Variant, allow_duplicates: bool = false) -> bool:
	return _array_op_dispatch(kind, id, field, "append", values, allow_duplicates)


## Prepend values to an Array field. Same de-dup semantics as append; the
## resulting prefix order matches the input order (prepend([a, b]) on [c]
## yields [a, b, c]).
func prepend(kind: String, id: Variant, field: String, values: Variant, allow_duplicates: bool = false) -> bool:
	return _array_op_dispatch(kind, id, field, "prepend", values, allow_duplicates)


## Remove values from an Array field. Removes ALL matching occurrences.
## Silent skip if a value isn't present (idempotent).
func remove_from(kind: String, id: Variant, field: String, values: Variant) -> bool:
	return _array_op_dispatch(kind, id, field, "remove_from", values, false)


# Shared dispatcher for append / prepend / remove_from. Mirrors patch()'s
# kind-by-kind routing exactly: kinds that support patch on Resource fields
# get a per-kind helper here; kinds with non-Resource entries (scenes,
# loot, shelters, etc.) get a warn-and-return-false branch.
func _array_op_dispatch(kind: String, id: Variant, field: String, op: String, values: Variant, allow_duplicates: bool) -> bool:
	if id is String and id == "":
		push_warning("[Registry] %s(%s, ...) called with empty id" % [op, kind])
		return false
	if field == "":
		push_warning("[Registry] %s(%s, ...) called with empty field" % [op, kind])
		return false
	var arr: Array = _coerce_to_array(values)
	if arr.is_empty():
		push_warning("[Registry] %s('%s', ...): empty values is a no-op" % [op, kind])
		return false
	match kind:
		"items":
			if not (id is String):
				push_warning("[Registry] %s('items', ...): id must be a String" % op)
				return false
			match op:
				"append":      return _append_item(id, field, arr, allow_duplicates)
				"prepend":     return _prepend_item(id, field, arr, allow_duplicates)
				"remove_from": return _remove_from_item(id, field, arr)
		"sounds":
			if not (id is String):
				push_warning("[Registry] %s('sounds', ...): id must be a String" % op)
				return false
			match op:
				"append":      return _append_sound(id, field, arr, allow_duplicates)
				"prepend":     return _prepend_sound(id, field, arr, allow_duplicates)
				"remove_from": return _remove_from_sound(id, field, arr)
		"recipes":
			match op:
				"append":      return _append_recipe(id, field, arr, allow_duplicates)
				"prepend":     return _prepend_recipe(id, field, arr, allow_duplicates)
				"remove_from": return _remove_from_recipe(id, field, arr)
		"events":
			match op:
				"append":      return _append_event(id, field, arr, allow_duplicates)
				"prepend":     return _prepend_event(id, field, arr, allow_duplicates)
				"remove_from": return _remove_from_event(id, field, arr)
		"trader_tasks":
			match op:
				"append":      return _append_trader_task(id, field, arr, allow_duplicates)
				"prepend":     return _prepend_trader_task(id, field, arr, allow_duplicates)
				"remove_from": return _remove_from_trader_task(id, field, arr)
		"resources":
			if not (id is String):
				push_warning("[Registry] %s('resources', ...): id must be a res:// path String" % op)
				return false
			match op:
				"append":      return _append_resource(id, field, arr, allow_duplicates)
				"prepend":     return _prepend_resource(id, field, arr, allow_duplicates)
				"remove_from": return _remove_from_resource(id, field, arr)
		"inputs":
			push_warning("[Registry] %s: 'inputs' has no Array-typed fields (display_label / default_event / deadzone are scalars; use patch instead)" % op)
			return false
		"scene_paths":
			push_warning("[Registry] %s: 'scene_paths' has no Array-typed fields (entries are path / Resource scalars; use patch instead)" % op)
			return false
		"scene_nodes":
			push_warning("[Registry] %s: 'scene_nodes' patches store literal property values applied on scene-load; Array-merge isn't supported (read the property in a hook and patch the merged value instead)" % op)
			return false
		"scenes":
			push_warning("[Registry] %s: 'scenes' doesn't support array ops (scenes are monolithic PackedScenes)" % op)
			return false
		"loot":
			push_warning("[Registry] %s: 'loot' doesn't support array ops (loot entries are ItemData references; use the items registry instead)" % op)
			return false
		"trader_pools":
			push_warning("[Registry] %s: 'trader_pools' doesn't support array ops (entries are boolean flags)" % op)
			return false
		"shelters":
			push_warning("[Registry] %s: 'shelters' doesn't support array ops (entries are bare strings)" % op)
			return false
		"maps":
			push_warning("[Registry] %s: 'maps' doesn't support array ops (entries are bare strings)" % op)
			return false
		"random_scenes":
			push_warning("[Registry] %s: 'random_scenes' doesn't support array ops (entries are bare paths)" % op)
			return false
		"ai_types":
			push_warning("[Registry] %s: 'ai_types' doesn't support array ops (entries are {scene, zone} refs)" % op)
			return false
		"ai_loadouts":
			push_warning("[Registry] %s: 'ai_loadouts' doesn't support array ops (entries are flat dicts; use override to replace)" % op)
			return false
		"fish_species":
			push_warning("[Registry] %s: 'fish_species' doesn't support array ops (entries are {scene, pool_id} refs)" % op)
			return false
		"weapons", "magazines", "attachments":
			push_warning("[Registry] %s: '%s' is a pure aggregator -- use the underlying primitive (e.g. %s('items', ...))" % [op, kind, op])
			return false
		_:
			push_warning("[Registry] %s: unknown registry '%s'" % [op, kind])
			return false
	return false


## Batched form of register(). `entries` is `{id: data, ...}`. Fans out
## to register() per entry; failures are isolated (one bad id doesn't stop
## the others). Returns `{ok: bool, results: {id: bool, ...}}`. `ok` is
## true only when every entry succeeded.
func register_many(kind: String, entries: Dictionary) -> Dictionary:
	var results: Dictionary = {}
	var all_ok := true
	for id in entries.keys():
		var ok: bool = register(kind, id, entries[id])
		results[id] = ok
		if not ok:
			all_ok = false
	return {"ok": all_ok, "results": results}


## Batched form of override(). Same shape as register_many.
func override_many(kind: String, entries: Dictionary) -> Dictionary:
	var results: Dictionary = {}
	var all_ok := true
	for id in entries.keys():
		var ok: bool = override(kind, id, entries[id])
		results[id] = ok
		if not ok:
			all_ok = false
	return {"ok": all_ok, "results": results}


## Batched form of patch(). `entries` is `{id: fields_dict, ...}`.
func patch_many(kind: String, entries: Dictionary) -> Dictionary:
	var results: Dictionary = {}
	var all_ok := true
	for id in entries.keys():
		var ok: bool = patch(kind, id, entries[id])
		results[id] = ok
		if not ok:
			all_ok = false
	return {"ok": all_ok, "results": results}


## Batched form of append(). `entries` is `{id: values, ...}` where values
## is a single value or Array. Same field across all entries (most common
## case); use individual append() calls if you need different fields per id.
func append_many(kind: String, field: String, entries: Dictionary, allow_duplicates: bool = false) -> Dictionary:
	var results: Dictionary = {}
	var all_ok := true
	for id in entries.keys():
		var ok: bool = append(kind, id, field, entries[id], allow_duplicates)
		results[id] = ok
		if not ok:
			all_ok = false
	return {"ok": all_ok, "results": results}


## Batched form of prepend(). Same shape as append_many.
func prepend_many(kind: String, field: String, entries: Dictionary, allow_duplicates: bool = false) -> Dictionary:
	var results: Dictionary = {}
	var all_ok := true
	for id in entries.keys():
		var ok: bool = prepend(kind, id, field, entries[id], allow_duplicates)
		results[id] = ok
		if not ok:
			all_ok = false
	return {"ok": all_ok, "results": results}


## Batched form of remove_from(). Same shape as append_many.
func remove_from_many(kind: String, field: String, entries: Dictionary) -> Dictionary:
	var results: Dictionary = {}
	var all_ok := true
	for id in entries.keys():
		var ok: bool = remove_from(kind, id, field, entries[id])
		results[id] = ok
		if not ok:
			all_ok = false
	return {"ok": all_ok, "results": results}


## Batched form of revert(). `entries` is `{id: fields_array, ...}`
## where fields_array can be empty (full revert of that id) or a list
## of field names.
func revert_many(kind: String, entries: Dictionary) -> Dictionary:
	var results: Dictionary = {}
	var all_ok := true
	for id in entries.keys():
		var fields_arg: Array = entries[id] if entries[id] is Array else []
		var ok: bool = revert(kind, id, fields_arg)
		results[id] = ok
		if not ok:
			all_ok = false
	return {"ok": all_ok, "results": results}


## Batched form of remove(). `ids` is an Array of String ids. Per-id
## results keyed by id.
func remove_many(kind: String, ids: Array) -> Dictionary:
	var results: Dictionary = {}
	var all_ok := true
	for id in ids:
		var sid := String(id)
		var ok: bool = remove(kind, sid)
		results[sid] = ok
		if not ok:
			all_ok = false
	return {"ok": all_ok, "results": results}



## True iff `id` resolves to either a mod registration or a vanilla entry.
func has_entry(kind: String, id: String) -> bool:
	match kind:
		"items": return _lookup_item(id) != null
		"loot":
			var reg: Dictionary = _registry_registered.get("loot", {})
			return reg.has(id)
		"sounds": return _lookup_sound(id) != null
		"events":
			# Read from the vanilla-side _id_index injected by
			# events_index_transform. Includes vanilla events (file-stem
			# keyed) and mod events (mod-supplied id).
			var events_res: Resource = load("res://Events/Events.tres")
			return events_res != null and "_id_index" in events_res and events_res._id_index.has(id)
		"recipes":
			# Read from the vanilla-side _id_index injected by
			# recipes_index_transform. Includes vanilla recipes (file-stem
			# keyed) and mod recipes (mod-supplied id). _registry_registered
			# is no longer the source of truth here; it stays as the
			# "is-this-mod-added" flag for the include_vanilla=false filter.
			var recipes: Resource = load("res://Crafting/Recipes.tres")
			return recipes != null and "_id_index" in recipes and recipes._id_index.has(id)
		"trader_pools":
			var reg: Dictionary = _registry_registered.get("trader_pools", {})
			return reg.has(id)
		"trader_tasks":
			# Tasks are scoped to a TraderData. Walk all four traders'
			# _id_index until the id resolves (vanilla file-stems) or a
			# mod-supplied handle. Mod registrations also live in
			# _registry_registered (see register_trader_task), but the
			# index is the canonical lookup.
			for path in ["res://Traders/Generalist/Generalist.tres", "res://Traders/Doctor/Doctor.tres", "res://Traders/Gunsmith/Gunsmith.tres"]:
				var t: Resource = load(path)
				if t != null and "_id_index" in t and t._id_index.has(id):
					return true
			return false
		"inputs": return InputMap.has_action(id)
		"scenes":
			var db: Node = _database_node()
			if db == null:
				return false
			if "_rtv_mod_scenes" in db and db._rtv_mod_scenes.has(id):
				return true
			return _scene_exists_in_vanilla(db, id)
		"scene_nodes":
			var patched: Dictionary = _registry_patched.get("scene_nodes", {})
			return patched.has(id)
		"scene_paths":
			var ldr: Node = _loader_node()
			if ldr == null:
				return false
			if "_rtv_mod_scene_paths" in ldr and ldr._rtv_mod_scene_paths.has(id):
				return true
			return _vanilla_scene_const_exists(ldr, id)
		"shelters":
			# True iff id resolves AND was registered as a shelter (not a map).
			var reg: Dictionary = _registry_registered.get("shelters", {})
			var entry: Variant = reg.get(id)
			if entry is Dictionary and entry.get("kind", "shelters") == "shelters":
				return true
			# Fall back to vanilla shelters list (no kind tag, all shelters).
			var ldr: Node = _loader_node()
			return ldr != null and id in ldr.shelters and not reg.has(id)
		"maps":
			# Maps live in the same bucket; filter by kind tag.
			var reg: Dictionary = _registry_registered.get("shelters", {})
			var entry: Variant = reg.get(id)
			return entry is Dictionary and entry.get("kind", "shelters") == "maps"
		"random_scenes":
			var reg: Dictionary = _registry_registered.get("random_scenes", {})
			return reg.has(id)
		"ai_types":
			var reg: Dictionary = _registry_registered.get("ai_types", {})
			return reg.has(id)
		"ai_loadouts":
			var reg: Dictionary = _registry_registered.get("ai_loadouts", {})
			return reg.has(id)
		"fish_species":
			var reg: Dictionary = _registry_registered.get("fish_species", {})
			return reg.has(id)
		_:
			push_warning("[Registry] has_entry: '%s' not yet implemented in crabby" % kind)
			return false


## Resolve `id` to its current entry (mod registration > vanilla). Returns
## null when nothing matches.
func get_entry(kind: String, id: String) -> Variant:
	match kind:
		"items": return _lookup_item(id)
		"loot":
			var reg: Dictionary = _registry_registered.get("loot", {})
			return reg.get(id)
		"sounds": return _lookup_sound(id)
		"events":
			# Read from vanilla-side _id_index. Index value is the
			# EventData ref directly (no wrapper Dict).
			var events_res: Resource = load("res://Events/Events.tres")
			if events_res == null or not ("_id_index" in events_res):
				return null
			return events_res._id_index.get(id)
		"recipes":
			# Read from vanilla-side _id_index. Each entry is
			# {recipe, category}; the RecipeData ref is returned
			# directly to match the previous get_entry contract.
			var recipes: Resource = load("res://Crafting/Recipes.tres")
			if recipes == null or not ("_id_index" in recipes):
				return null
			var entry: Variant = recipes._id_index.get(id)
			if entry is Dictionary and entry.has("recipe"):
				return entry["recipe"]
			return null
		"trader_pools":
			var reg: Dictionary = _registry_registered.get("trader_pools", {})
			return reg.get(id)
		"trader_tasks":
			# Walk all four trader resources' _id_index. Vanilla tasks
			# indexed by file-stem; mod tasks by their handle.
			for path in ["res://Traders/Generalist/Generalist.tres", "res://Traders/Doctor/Doctor.tres", "res://Traders/Gunsmith/Gunsmith.tres"]:
				var t: Resource = load(path)
				if t != null and "_id_index" in t and t._id_index.has(id):
					return t._id_index[id]
			return null
		"inputs":
			var reg: Dictionary = _registry_registered.get("inputs", {})
			return reg.get(id)
		"scenes":
			var db: Node = _database_node()
			if db == null:
				return null
			# Database's injected _get() handles override > mod > vanilla
			# precedence; just delegate.
			return db.get(id)
		"scene_nodes":
			var patched: Dictionary = _registry_patched.get("scene_nodes", {})
			return patched.get(id)
		"scene_paths":
			var ldr: Node = _loader_node()
			if ldr == null:
				return null
			if "_rtv_override_scene_paths" in ldr and ldr._rtv_override_scene_paths.has(id):
				return ldr._rtv_override_scene_paths[id]
			if "_rtv_mod_scene_paths" in ldr and ldr._rtv_mod_scene_paths.has(id):
				return ldr._rtv_mod_scene_paths[id]
			return null
		"shelters":
			var reg: Dictionary = _registry_registered.get("shelters", {})
			var entry: Variant = reg.get(id)
			# Filter cross-surface lookups so get_entry('shelters', X)
			# returns null when X was registered as a map.
			if entry is Dictionary and entry.get("kind", "shelters") != "shelters":
				return null
			return entry
		"maps":
			var reg: Dictionary = _registry_registered.get("shelters", {})
			var entry: Variant = reg.get(id)
			if entry is Dictionary and entry.get("kind", "shelters") == "maps":
				return entry
			return null
		"random_scenes":
			var reg: Dictionary = _registry_registered.get("random_scenes", {})
			return reg.get(id)
		"ai_types":
			var reg: Dictionary = _registry_registered.get("ai_types", {})
			return reg.get(id)
		"ai_loadouts":
			var reg: Dictionary = _registry_registered.get("ai_loadouts", {})
			return reg.get(id)
		"fish_species":
			var reg: Dictionary = _registry_registered.get("fish_species", {})
			return reg.get(id)
		_:
			push_warning("[Registry] get_entry: '%s' not yet implemented in crabby" % kind)
			return null


# ---- read API: has / keys / list / find ----
#
# Iteration helpers over the union of mod registrations + vanilla
# entries. `include_vanilla=true` is the default so mods asking
# "what items exist" see the full catalog. Pure-mod registries (loot,
# trader_pools, etc.) have empty vanilla side; the mod-entries dict is
# the complete picture for those.

## True iff `id` resolves in the registry. Like `has_entry` but with
## an explicit `include_vanilla` toggle so mods can ask "is this a
## mod-only id?"
func has(kind: String, id: String, include_vanilla: bool = true) -> bool:
	# Migrated kinds (recipes/events/items/trader_tasks) record mod-added
	# ids in the lightweight Set; their data lives on the vanilla
	# `_id_index`. Other kinds keep their data in `_registry_registered`.
	var reg: Dictionary = _registry_registered.get(kind, {})
	if reg.has(id) or _mod_added_has(kind, id):
		return true
	if not include_vanilla:
		return false
	var vanilla: Dictionary = _enumerate_vanilla(kind)
	return vanilla.has(id)


## Just the ids in this registry, as a typed String array. Cheaper than
## `list().keys()` because the merged values dict isn't materialized
## when the caller doesn't need it.
func keys(kind: String, include_vanilla: bool = true) -> Array[String]:
	var out: Array[String] = []
	var seen: Dictionary = {}
	if include_vanilla:
		var vanilla: Dictionary = _enumerate_vanilla(kind)
		for k in vanilla.keys():
			out.append(String(k))
			seen[k] = true
	# Migrated kinds: walk `_mod_added_set`. Other kinds: walk the
	# `_registry_registered` data dict. Both contribute to the union.
	var reg: Dictionary = _registry_registered.get(kind, {})
	for k in reg.keys():
		if not seen.has(k):
			out.append(String(k))
			seen[k] = true
	var mod_added: Dictionary = _mod_added_set(kind)
	for k in mod_added.keys():
		if not seen.has(k):
			out.append(String(k))
	return out


## Full id → entry mapping. Mod entries override vanilla on id collision
## (matches `get_entry` precedence).
func list(kind: String, include_vanilla: bool = true) -> Dictionary:
	var out: Dictionary = {}
	if include_vanilla:
		out = _enumerate_vanilla(kind).duplicate()
	var reg: Dictionary = _registry_registered.get(kind, {})
	for k in reg.keys():
		out[k] = reg[k]
	# Migrated kinds: pull mod-added entries from get_entry (which
	# routes through the vanilla `_id_index`). Skip when include_vanilla
	# was true; the vanilla enumeration already included them.
	if not include_vanilla:
		var mod_added: Dictionary = _mod_added_set(kind)
		for k in mod_added.keys():
			var entry: Variant = get_entry(kind, String(k))
			if entry != null:
				out[k] = entry
	return out


## Filtered iteration. Predicate signature: `func(entry) -> bool`.
## Returns an Array of `{id, entry}` Dictionaries for every match. The
## id is included so callers don't need a separate lookup.
func find(kind: String, predicate: Callable, include_vanilla: bool = true) -> Array:
	var out: Array = []
	var entries: Dictionary = list(kind, include_vanilla)
	for id in entries.keys():
		var entry: Variant = entries[id]
		if entry == null:
			continue
		if bool(predicate.call(entry)):
			out.append({"id": String(id), "entry": entry})
	return out


## Per-registry vanilla source enumerator. Returns id -> entry for every
## vanilla content item the registry tracks. Pure-mod registries
## (loot, trader_pools, scene_paths-mod-only, etc.) return {}; their
## entries are inherently mod-side only.
func _enumerate_vanilla(kind: String) -> Dictionary:
	match kind:
		"items":
			# Vanilla items live in LT_Master._id_index (injected by
			# loot_table_index_transform), keyed by ItemData.file. Filter
			# OUT mod-registered ids to avoid double-counting in the merge.
			var out: Dictionary = {}
			var master: Resource = load("res://Loot/LT_Master.tres")
			if master == null or not ("_id_index" in master):
				return out
			var mod_reg: Dictionary = _mod_added_set("items")
			for key in master._id_index.keys():
				if mod_reg.has(key):
					continue
				out[String(key)] = master._id_index[key]
			return out
		"scenes":
			# Vanilla scenes are const declarations on Database.gd.
			# Walk the script's constant map.
			var out: Dictionary = {}
			var db: Node = _database_node()
			if db == null or db.get_script() == null:
				return out
			var consts: Dictionary = db.get_script().get_script_constant_map()
			for k in consts.keys():
				var v: Variant = consts[k]
				if v is PackedScene:
					out[String(k)] = v
			return out
		"scene_paths":
			# Vanilla scene-path consts on Loader.gd. Same const-map walk
			# but values are res:// path Strings rather than PackedScenes.
			var out: Dictionary = {}
			var ldr: Node = _loader_node()
			if ldr == null or ldr.get_script() == null:
				return out
			var consts: Dictionary = ldr.get_script().get_script_constant_map()
			for k in consts.keys():
				var v: Variant = consts[k]
				if v is String and String(v).begins_with("res://"):
					out[String(k)] = v
			return out
		"shelters":
			# Vanilla shelters are entries in `_rtv_vanilla_shelters`
			# (snapshotted by the loader_transform rewriter). Each
			# shelter "entry" is just its name; returns name -> name
			# for shape consistency with other registries.
			var out: Dictionary = {}
			var ldr: Node = _loader_node()
			if ldr == null or not ("_rtv_vanilla_shelters" in ldr):
				return out
			for n in ldr._rtv_vanilla_shelters:
				out[String(n)] = String(n)
			return out
		"recipes":
			# Vanilla recipes live in Recipes.tres's _id_index (injected
			# by recipes_index_transform), keyed by file-stem. Filters
			# OUT mod-registered ids so the merge in has/keys/list with
			# _registry_registered doesn't double-count.
			var out: Dictionary = {}
			var recipes: Resource = load("res://Crafting/Recipes.tres")
			if recipes == null or not ("_id_index" in recipes):
				return out
			var mod_reg: Dictionary = _mod_added_set("recipes")
			for key in recipes._id_index.keys():
				if mod_reg.has(key):
					continue
				var entry: Variant = recipes._id_index[key]
				if entry is Dictionary and entry.has("recipe"):
					out[String(key)] = entry["recipe"]
			return out
		"events":
			# Vanilla events live in Events.tres's _id_index (injected
			# by events_index_transform). Filter OUT mod-registered ids
			# so the merge in has/keys/list with _registry_registered
			# doesn't double-count.
			var out: Dictionary = {}
			var events_res: Resource = load("res://Events/Events.tres")
			if events_res == null or not ("_id_index" in events_res):
				return out
			var mod_reg: Dictionary = _mod_added_set("events")
			for key in events_res._id_index.keys():
				if mod_reg.has(key):
					continue
				out[String(key)] = events_res._id_index[key]
			return out
		"trader_tasks":
			# Vanilla tasks live on each TraderData's _id_index (injected
			# by trader_data_index_transform). Walk all four resources and
			# merge; filter OUT mod-registered ids so the merge in
			# has/keys/list with _registry_registered doesn't double-count.
			var out: Dictionary = {}
			var mod_reg: Dictionary = _mod_added_set("trader_tasks")
			for path in ["res://Traders/Generalist/Generalist.tres", "res://Traders/Doctor/Doctor.tres", "res://Traders/Gunsmith/Gunsmith.tres"]:
				var t: Resource = load(path)
				if t == null or not ("_id_index" in t):
					continue
				for key in t._id_index.keys():
					if mod_reg.has(key):
						continue
					out[String(key)] = t._id_index[key]
			return out
		# Pure-mod registries: vanilla side is empty. The mod-entries
		# dict in _registry_registered is the complete picture.
		"loot", "trader_pools", "sounds", \
		"inputs", "random_scenes", "ai_types", "ai_loadouts", "fish_species", "resources", \
		"scene_nodes", "weapons", "magazines", "attachments":
			return {}
		_:
			push_warning("[Registry] _enumerate_vanilla: unknown registry '%s'" % kind)
			return {}

