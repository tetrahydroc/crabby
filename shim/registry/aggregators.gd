# ============================================================================
# Aggregators - fan-out helpers
# ============================================================================
#
# These wrap several primitive registries into a single declarative dict.
# All return Dictionary with granular per-step success bools. The
# Registry-const dispatch above (`register('weapons', ...)` etc.) collapses
# to a single bool; callers wanting partial-failure detail use the public
# `register_weapon` / `register_magazine` / `register_attachment` /
# `register_item` / `register_furniture` methods directly.

# -------- weapons --------

## Required keys: item_path, scene_path, rig_path. Optional: icon_path,
## magazines (Array of inline bundles or id strings), fits_attachments
## (Array of id strings), loot_tables (Array of table names).
func _register_weapon(id: String, data: Variant) -> Dictionary:
	var result: Dictionary = {
		"ok": false,
		"items": false,
		"scene": false,
		"rig": false,
		"magazines": [],
		"fits_attachments": [],
		"fits_attachments_failed": [],
		"loot_count": 0,
		# `ai_loadout` is left as null when the data dict didn't request
		# AI loadout registration; set to true / false depending on the
		# registration outcome when it did. Caller can distinguish "not
		# requested" from "requested but failed" without parsing logs.
		"ai_loadout": null,
	}
	if not (data is Dictionary):
		push_warning("[Registry] register('weapons', '%s', ...) expects Dictionary" % id)
		return result
	var d: Dictionary = data
	for required in ["item_path", "scene_path", "rig_path"]:
		if not d.has(required):
			push_warning("[Registry] register('weapons', '%s'): missing required key '%s'" % [id, required])
			return result
	# Step 1: load + register the weapon ItemData.
	var weapon_item: Resource = load(d["item_path"])
	if weapon_item == null:
		push_warning("[Registry] register('weapons', '%s'): failed to load item from '%s'" % [id, d["item_path"]])
		return result
	if d.has("icon_path"):
		_apply_icon(weapon_item, d["icon_path"], id)
	result["items"] = _register_item(id, weapon_item)
	if not result["items"]:
		# Item registration is the foundation; abort if it failed, the
		# rest of the fan-out has nothing to attach `compatible` to.
		return result
	# Step 2: world scene.
	var world_scene: Resource = load(d["scene_path"])
	if world_scene != null:
		result["scene"] = _register_scene(id, world_scene)
	# Step 3: rig scene. Convention: "<weapon_id>_Rig".
	var rig_scene: Resource = load(d["rig_path"])
	if rig_scene != null:
		result["rig"] = _register_scene(id + "_Rig", rig_scene)
	# Step 4: magazines (mixed array of inline bundles + id strings).
	var compatible_additions: Array = []
	if d.has("magazines") and d["magazines"] is Array:
		for entry in d["magazines"]:
			var mag_result: Dictionary = _register_weapon_magazine_entry(entry)
			result["magazines"].append(mag_result)
			if mag_result.get("item_data") != null:
				compatible_additions.append(mag_result["item_data"])
	# Step 5: fits_attachments (id-only refs through _lookup_item).
	if d.has("fits_attachments") and d["fits_attachments"] is Array:
		for att_id in d["fits_attachments"]:
			if not (att_id is String):
				continue
			var att_item: Resource = _lookup_item(att_id)
			if att_item == null:
				result["fits_attachments_failed"].append(att_id)
				push_warning("[Registry] register('weapons', '%s'): fits_attachments id '%s' didn't resolve (typo? not registered yet?)" % [id, att_id])
				continue
			result["fits_attachments"].append(att_id)
			if not (att_item in compatible_additions):
				compatible_additions.append(att_item)
	# Step 6: patch the weapon's `compatible` array in one shot.
	if not compatible_additions.is_empty():
		var existing: Array = []
		if "compatible" in weapon_item:
			var cur: Variant = weapon_item.get("compatible")
			if cur is Array:
				existing = (cur as Array).duplicate()
		for add in compatible_additions:
			if not (add in existing):
				existing.append(add)
		_patch_item(id, {"compatible": existing})
	# Step 7: loot tables. One register('loot', ...) call per table.
	if d.has("loot_tables") and d["loot_tables"] is Array:
		for table_name in d["loot_tables"]:
			if not (table_name is String):
				continue
			var loot_id: String = "%s_in_%s" % [id, table_name]
			if _register_loot(loot_id, {"item": weapon_item, "table": String(table_name)}):
				result["loot_count"] = int(result["loot_count"]) + 1
	# Step 8: AI loadout. Optional. Auto-uses the weapon's scene_path
	# and id; the caller's ai_loadout dict adds ai_types / chance /
	# replace. Failure is reported in result.ai_loadout but does not
	# fail the whole weapon register; the weapon still spawns as
	# loot, it just won't be carried by AI.
	if d.has("ai_loadout"):
		var al: Variant = d["ai_loadout"]
		if not (al is Dictionary):
			push_warning("[Registry] register('weapons', '%s'): ai_loadout must be a Dictionary, got %s" % [id, typeof(al)])
			result["ai_loadout"] = false
		else:
			# Compose the ai_loadouts entry: pin weapon_scene to this
			# weapon's already-loaded scene resource so it isn't re-loaded
			# and doesn't risk a scene-id resolution miss.
			var loadout_data: Dictionary = (al as Dictionary).duplicate()
			loadout_data["weapon_scene"] = world_scene
			result["ai_loadout"] = _register_ai_loadout(id, loadout_data)
	# Final ok: items+scene+rig succeeded, no fits failures (magazines
	# tracked separately; caller can drill in). ai_loadout doesn't
	# gate ok since "weapon registered, loadout failed" is a partial-
	# success the mod author can choose to act on.
	result["ok"] = result["items"] and result["scene"] and result["rig"] \
			and result["fits_attachments_failed"].is_empty()
	return result


## Per-magazine processing inside register_weapon. Accepts Dictionary
## (inline bundle) or String (id ref). Returns:
##   {id, ok, item_data, items?, scene?, loot_count?}
func _register_weapon_magazine_entry(entry: Variant) -> Dictionary:
	if entry is String:
		var mag: Resource = _lookup_item(entry)
		if mag == null:
			push_warning("[Registry] register_weapon: magazine id '%s' didn't resolve (typo? not registered yet?)" % entry)
			return {"id": entry, "ok": false, "item_data": null}
		return {"id": entry, "ok": true, "item_data": mag}
	if entry is Dictionary:
		var d: Dictionary = entry
		if not d.has("id") or not (d["id"] is String):
			push_warning("[Registry] register_weapon: inline magazine missing 'id' string key")
			return {"id": "", "ok": false, "item_data": null}
		var sub: Dictionary = _register_magazine(d["id"], d)
		var sub_item: Resource = _lookup_item(d["id"])
		return {
			"id": d["id"],
			"ok": sub["ok"],
			"item_data": sub_item,
			"items": sub.get("items", false),
			"scene": sub.get("scene", false),
			"loot_count": sub.get("loot_count", 0),
		}
	push_warning("[Registry] register_weapon: magazine entry must be a Dictionary or String id, got %s" % typeof(entry))
	return {"id": "", "ok": false, "item_data": null}


# -------- magazines + attachments (shared shape) --------

func _register_magazine(id: String, data: Variant) -> Dictionary:
	return _register_compat_item(id, data, "magazines")


func _register_attachment(id: String, data: Variant) -> Dictionary:
	return _register_compat_item(id, data, "attachments")


## Shared implementation for magazines + attachments. Both register an
## item + scene + optional loot, then patch `compatible` on each weapon
## listed in fits_weapons.
func _register_compat_item(id: String, data: Variant, label: String) -> Dictionary:
	var result: Dictionary = {
		"ok": false,
		"items": false,
		"scene": false,
		"fits_weapons": [],
		"fits_weapons_failed": [],
		"loot_count": 0,
	}
	if not (data is Dictionary):
		push_warning("[Registry] register('%s', '%s', ...) expects Dictionary" % [label, id])
		return result
	var d: Dictionary = data
	for required in ["item_path", "scene_path"]:
		if not d.has(required):
			push_warning("[Registry] register('%s', '%s'): missing required key '%s'" % [label, id, required])
			return result
	var item_data: Resource = load(d["item_path"])
	if item_data == null:
		push_warning("[Registry] register('%s', '%s'): failed to load item from '%s'" % [label, id, d["item_path"]])
		return result
	if d.has("icon_path"):
		_apply_icon(item_data, d["icon_path"], id)
	result["items"] = _register_item(id, item_data)
	if not result["items"]:
		return result
	var scene: Resource = load(d["scene_path"])
	if scene != null:
		result["scene"] = _register_scene(id, scene)
	# fits_weapons: append `item_data` to each target weapon's `compatible`.
	if d.has("fits_weapons") and d["fits_weapons"] is Array:
		for weapon_id in d["fits_weapons"]:
			if not (weapon_id is String):
				continue
			var weapon_item: Resource = _lookup_item(weapon_id)
			if weapon_item == null:
				result["fits_weapons_failed"].append(weapon_id)
				push_warning("[Registry] register('%s', '%s'): fits_weapons id '%s' didn't resolve" % [label, id, weapon_id])
				continue
			var existing: Array = []
			if "compatible" in weapon_item:
				var cur: Variant = weapon_item.get("compatible")
				if cur is Array:
					existing = (cur as Array).duplicate()
			if not (item_data in existing):
				existing.append(item_data)
			if _patch_item(weapon_id, {"compatible": existing}):
				result["fits_weapons"].append(weapon_id)
			else:
				result["fits_weapons_failed"].append(weapon_id)
	if d.has("loot_tables") and d["loot_tables"] is Array:
		for table_name in d["loot_tables"]:
			if not (table_name is String):
				continue
			var loot_id: String = "%s_in_%s" % [id, table_name]
			if _register_loot(loot_id, {"item": item_data, "table": String(table_name)}):
				result["loot_count"] = int(result["loot_count"]) + 1
	result["ok"] = result["items"] and result["scene"] and result["fits_weapons_failed"].is_empty()
	return result


# -------- generic item bundle --------

## Schema:
##   item_path     - required, res:// to the .tres ItemData
##   scene_path    - optional, res:// to the world .tscn (skip for items
##                   that only ever exist as inventory entries)
##   icon_path     - optional, image path; loaded + assigned to item.icon
##   loot_tables   - optional, list of table names (one register('loot') each)
##   trader_pools  - optional, list of trader names ("Generalist", "Doctor",
##                   "Gunsmith", "Grandma") to flip the matching ItemData flag
func _register_item_bundle(id: String, data: Variant) -> Dictionary:
	var result: Dictionary = {
		"ok": false,
		"items": false,
		"scene": true,  # default true so missing scene_path doesn't fail ok
		"loot_count": 0,
		"trader_pool_count": 0,
		"trader_pools": [],
		"trader_pools_failed": [],
	}
	if not (data is Dictionary):
		push_warning("[Registry] register_item('%s', ...) expects Dictionary" % id)
		return result
	var d: Dictionary = data
	if not d.has("item_path"):
		push_warning("[Registry] register_item('%s'): missing required key 'item_path'" % id)
		return result
	var item_data: Resource = load(d["item_path"])
	if item_data == null:
		push_warning("[Registry] register_item('%s'): failed to load item from '%s'" % [id, d["item_path"]])
		return result
	if d.has("icon_path"):
		_apply_icon(item_data, d["icon_path"], id)
	result["items"] = _register_item(id, item_data)
	if not result["items"]:
		return result
	# scene: only override the default-true when a path was provided.
	if d.has("scene_path"):
		var scene: Resource = load(d["scene_path"])
		if scene != null:
			result["scene"] = _register_scene(id, scene)
		else:
			result["scene"] = false
			push_warning("[Registry] register_item('%s'): failed to load scene from '%s'" % [id, d["scene_path"]])
	if d.has("loot_tables") and d["loot_tables"] is Array:
		for table_name in d["loot_tables"]:
			if not (table_name is String):
				continue
			var loot_id: String = "%s_in_%s" % [id, table_name]
			if _register_loot(loot_id, {"item": item_data, "table": String(table_name)}):
				result["loot_count"] = int(result["loot_count"]) + 1
	if d.has("trader_pools") and d["trader_pools"] is Array:
		for pool_name in d["trader_pools"]:
			if not (pool_name is String):
				continue
			var pool_id: String = "%s_in_pool_%s" % [id, pool_name]
			if _register_trader_pool(pool_id, {"item": item_data, "trader": String(pool_name)}):
				result["trader_pools"].append(String(pool_name))
				result["trader_pool_count"] = int(result["trader_pool_count"]) + 1
			else:
				result["trader_pools_failed"].append(String(pool_name))
	result["ok"] = result["items"] and result["scene"] and result["trader_pools_failed"].is_empty()
	return result


# -------- furniture bundle --------

## Furniture = ItemData with type="Furniture" + placed scene + traders.
## Differs from generic items in three ways:
##   - never spawns from loot pools
##   - obtained via trader supply (or task rewards)
##   - on purchase, vanilla routes it to the catalog grid
##     (Interface.gd branches on itemData.type == "Furniture")
##
## Schema:
##   item_path     - required (.tres ItemData with type="Furniture")
##   scene_path    - required (placed world .tscn)
##   icon_path     - optional
##   trader_pools  - optional, default ["Generalist"] + warn
##   recipe        - optional dict {input: Array[ItemData], time: float,
##                   audio?: AudioEvent}. Output is implicit.
func _register_furniture_bundle(id: String, data: Variant) -> Dictionary:
	var result: Dictionary = {
		"ok": false,
		"items": false,
		"scene": false,
		"trader_pool_count": 0,
		"trader_pools": [],
		"trader_pools_failed": [],
		"recipe": false,
	}
	if not (data is Dictionary):
		push_warning("[Registry] register_furniture('%s', ...) expects Dictionary" % id)
		return result
	var d: Dictionary = data
	for required in ["item_path", "scene_path"]:
		if not d.has(required):
			push_warning("[Registry] register_furniture('%s'): missing required key '%s'" % [id, required])
			return result
	if d.has("loot_tables"):
		push_warning("[Registry] register_furniture('%s'): loot_tables is not supported (furniture isn't loot-pool spawnable in vanilla; use trader_pools instead). Ignored." % id)
	var item_data: Resource = load(d["item_path"])
	if item_data == null:
		push_warning("[Registry] register_furniture('%s'): failed to load item from '%s'" % [id, d["item_path"]])
		return result
	if "type" in item_data and String(item_data.get("type")) != "Furniture":
		push_warning("[Registry] register_furniture('%s'): ItemData.type is '%s', expected 'Furniture'. Item won't be routed to the catalog grid on purchase. Fix the .tres or the player will get inventory items instead." % [id, item_data.get("type")])
	if d.has("icon_path"):
		_apply_icon(item_data, d["icon_path"], id)
	result["items"] = _register_item(id, item_data)
	if not result["items"]:
		return result
	var scene: Resource = load(d["scene_path"])
	if scene == null:
		push_warning("[Registry] register_furniture('%s'): failed to load scene from '%s'" % [id, d["scene_path"]])
	else:
		result["scene"] = _register_scene(id, scene)
	# Trader pools: default ["Generalist"] with a loud warn so unobtainable
	# furniture surfaces at register time, not "why doesn't this show up?"
	var pools: Array = []
	if d.has("trader_pools") and d["trader_pools"] is Array and not (d["trader_pools"] as Array).is_empty():
		pools = d["trader_pools"]
	else:
		pools = ["Generalist"]
		push_warning("[Registry] register_furniture('%s'): no trader_pools specified, defaulting to ['Generalist']. Furniture is only obtainable via traders." % id)
	for pool_name in pools:
		if not (pool_name is String):
			continue
		var pool_id: String = "%s_in_pool_%s" % [id, pool_name]
		if _register_trader_pool(pool_id, {"item": item_data, "trader": String(pool_name)}):
			result["trader_pools"].append(String(pool_name))
			result["trader_pool_count"] = int(result["trader_pool_count"]) + 1
		else:
			result["trader_pools_failed"].append(String(pool_name))
	# Optional crafting recipe.
	if d.has("recipe") and d["recipe"] is Dictionary:
		var rd: Dictionary = d["recipe"]
		if not (rd.has("input") and rd["input"] is Array) or (rd["input"] as Array).is_empty():
			push_warning("[Registry] register_furniture('%s'): recipe.input must be a non-empty array of ItemData" % id)
		else:
			var recipe: Resource = _build_furniture_recipe(id, item_data, rd)
			if recipe != null:
				var recipe_id: String = "%s_recipe" % id
				result["recipe"] = _register_recipe(recipe_id, {"recipe": recipe, "category": "furniture"})
	result["ok"] = result["items"] and result["scene"] and result["trader_pools_failed"].is_empty()
	return result


## Construct a fresh RecipeData from the modder's recipe dict. Output is
## implicit (the item being registered).
func _build_furniture_recipe(id: String, output_item: Resource, rd: Dictionary) -> Resource:
	var script: GDScript = load("res://Scripts/RecipeData.gd") as GDScript
	if script == null:
		push_warning("[Registry] register_furniture('%s'): failed to load RecipeData.gd; recipe skipped" % id)
		return null
	var recipe: Resource = script.new()
	recipe.set("name", String(rd.get("name", id)))
	recipe.set("time", float(rd.get("time", 1.0)))
	if rd.has("audio"):
		recipe.set("audio", rd["audio"])
	# RecipeData.input is `Array[ItemData]`; assigning an untyped Array
	# silently fails the typed-array check inside _register_recipe.
	var typed_input: Array[ItemData] = []
	for it in rd["input"]:
		if it is ItemData:
			typed_input.append(it)
		else:
			push_warning("[Registry] register_furniture('%s'): recipe.input contains non-ItemData entry; skipped" % id)
	if typed_input.is_empty():
		return null
	recipe.set("input", typed_input)
	var typed_output: Array[ItemData] = []
	if output_item is ItemData:
		typed_output.append(output_item)
	else:
		push_warning("[Registry] register_furniture('%s'): output ItemData isn't typed as ItemData; recipe skipped" % id)
		return null
	recipe.set("output", typed_output)
	for flag in ["heat", "workbench", "testbench", "shelter"]:
		if rd.has(flag):
			recipe.set(flag, bool(rd[flag]))
	return recipe


# -------- shared helpers --------

## Load image, convert to ImageTexture, assign to item_data.icon if the
## field exists. Best-effort: failures warn but don't abort the parent
## registration; icons are cosmetic.
func _apply_icon(item_data: Resource, icon_path: String, owner_id: String) -> void:
	if not _resource_has_property(item_data, "icon"):
		return
	var img: Image = Image.new()
	if img.load(icon_path) != OK:
		push_warning("[Registry] register: '%s' icon load failed for '%s'" % [owner_id, icon_path])
		return
	if img.get_size().x == 0 or img.get_size().y == 0:
		push_warning("[Registry] register: '%s' icon loaded but is empty (path resolved but size 0)" % owner_id)
		return
	item_data.icon = ImageTexture.create_from_image(img)
