# ---- loot kind ----
#
# Loot registrations append/swap entries on the `items: Array[ItemData]`
# of LootTable Resources. Godot's resource cache means every load() of
# the same .tres returns the same instance, so mutations propagate to
# every later consumer that reads the array, provided they haven't
# already snapshotted it locally. (Mods must register during their own
# `_ready()` before vanilla autoloads copy table contents into local
# buckets.)

## Map short table names → absolute res:// paths. Mods that pass an
## already-absolute path use it as-is. Mirrors vostok-mod-loader's
## `_LOOT_TABLE_PATHS` at registry/loot.gd:24.
const _LOOT_TABLE_PATHS := {
	"LT_Master": "res://Loot/LT_Master.tres",
	"LT_Airdrop": "res://Loot/Custom/LT_Airdrop.tres",
	"LT_Patient_Report": "res://Loot/Custom/LT_Patient_Report.tres",
	"LT_Punisher": "res://Loot/Custom/LT_Punisher.tres",
	"LT_Oil_Sample": "res://Loot/Custom/LT_Oil_Sample.tres",
	"LT_Weapons_01": "res://Loot/Tutorial/LT_Weapons_01.tres",
	"LT_Weapons_02": "res://Loot/Tutorial/LT_Weapons_02.tres",
	"LT_Weapons_03": "res://Loot/Tutorial/LT_Weapons_03.tres",
	"LT_Weapons_04": "res://Loot/Tutorial/LT_Weapons_04.tres",
	"LT_Ammo": "res://Loot/Tutorial/LT_Ammo.tres",
	"LT_Medical": "res://Loot/Tutorial/LT_Medical.tres",
	"LT_Equipment": "res://Loot/Tutorial/LT_Equipment.tres",
	"LT_Armor": "res://Loot/Tutorial/LT_Armor.tres",
	"LT_Grenades": "res://Loot/Tutorial/LT_Grenades.tres",
	"LT_Attachments": "res://Loot/Tutorial/LT_Attachments.tres",
	"LT_Items": "res://Loot/Tutorial/LT_Items.tres",
	"Kit_Colt": "res://Loot/Kits/Kit_Colt.tres",
	"Kit_Glock": "res://Loot/Kits/Kit_Glock.tres",
	"Kit_MP5K": "res://Loot/Kits/Kit_MP5K.tres",
	"Kit_Makarov": "res://Loot/Kits/Kit_Makarov.tres",
	"Kit_Mosin": "res://Loot/Kits/Kit_Mosin.tres",
	"Kit_Remington": "res://Loot/Kits/Kit_Remington.tres",
}


func _resolve_loot_table(table_ref: String) -> Resource:
	if table_ref == "":
		push_warning("[Registry] loot: empty table name")
		return null
	var path := table_ref
	if _LOOT_TABLE_PATHS.has(table_ref):
		path = _LOOT_TABLE_PATHS[table_ref]
	elif not table_ref.begins_with("res://"):
		push_warning("[Registry] loot: unknown table '%s' (not a known vanilla table name and not an absolute res:// path)" % table_ref)
		return null
	var res: Resource = load(path)
	if res == null:
		push_warning("[Registry] loot: couldn't load table at '%s'" % path)
		return null
	if not ("items" in res):
		push_warning("[Registry] loot: resource at '%s' has no `items` array (not a LootTable?)" % path)
		return null
	return res


## Validate `{item, table}` payload. Returns `[item, table_res]` or
## `[null, null]` on error (with a warning already issued).
func _validate_loot_data(id: String, verb: String, data: Variant) -> Array:
	if not (data is Dictionary):
		push_warning("[Registry] %s('loot', '%s', ...) expects Dictionary {item, table}, got %s" % [verb, id, typeof(data)])
		return [null, null]
	var d: Dictionary = data
	if not d.has("item") or not d.has("table"):
		push_warning("[Registry] %s('loot', '%s', ...) data dict missing 'item' or 'table' key" % [verb, id])
		return [null, null]
	var item: Variant = d["item"]
	if not (item is Resource) or not _looks_like_item_data(item):
		push_warning("[Registry] %s('loot', '%s'): item is not an ItemData Resource" % [verb, id])
		return [null, null]
	var table: Variant = d["table"]
	if not (table is String):
		push_warning("[Registry] %s('loot', '%s'): table must be a String (name or res:// path)" % [verb, id])
		return [null, null]
	var table_res: Resource = _resolve_loot_table(table)
	if table_res == null:
		return [null, null]
	return [item, table_res]


func _register_loot(id: String, data: Variant) -> bool:
	var reg: Dictionary = _registry_registered.get("loot", {})
	if reg.has(id):
		push_warning("[Registry] register('loot', '%s'): already registered (ids are mod-chosen handles, pick a unique one)" % id)
		return false
	var parts: Array = _validate_loot_data(id, "register", data)
	var item: Variant = parts[0]
	var table_res: Variant = parts[1]
	if item == null or table_res == null:
		return false
	if not _typed_array_accepts(table_res.items, item):
		push_warning("[Registry] register('loot', '%s'): item type doesn't match table's typed array" % id)
		return false
	# Idempotent guard: don't double-insert; mods should use override to swap.
	if item in table_res.items:
		push_warning("[Registry] register('loot', '%s'): item is already present in table; use override to swap an existing entry" % id)
		return false
	table_res.items.append(item)
	# Drive the table's _id_index (injected onto LootTable.gd by
	# loot_table_index_transform). Index key is the canonical item.file.
	var item_id: String = String(item.get("file") if item.get("file") != null else "")
	if item_id != "" and table_res.has_method("_index_add"):
		table_res._index_add(item_id, item)
	reg[id] = {"item": item, "table": data["table"], "table_res": table_res}
	_registry_registered["loot"] = reg
	return true


## Override swaps an existing ItemData in a table for a new one. Data
## payload extends register's shape with `replaces: ItemData`, the
## existing entry to swap out.
func _override_loot(id: String, data: Variant) -> bool:
	var ov: Dictionary = _registry_overridden.get("loot", {})
	if ov.has(id):
		push_warning("[Registry] override('loot', '%s'): already overridden (revert first to re-override)" % id)
		return false
	if not (data is Dictionary) or not data.has("replaces"):
		push_warning("[Registry] override('loot', '%s', ...) requires {item, table, replaces: ItemData}; 'replaces' is the existing entry to swap out" % id)
		return false
	var parts: Array = _validate_loot_data(id, "override", data)
	var new_item: Variant = parts[0]
	var table_res: Variant = parts[1]
	if new_item == null or table_res == null:
		return false
	var old_item: Variant = data["replaces"]
	if not (old_item is Resource) or not _looks_like_item_data(old_item):
		push_warning("[Registry] override('loot', '%s'): 'replaces' is not an ItemData Resource" % id)
		return false
	if not _typed_array_accepts(table_res.items, new_item):
		push_warning("[Registry] override('loot', '%s'): item type doesn't match table's typed array" % id)
		return false
	var idx: int = table_res.items.find(old_item)
	if idx < 0:
		push_warning("[Registry] override('loot', '%s'): 'replaces' item not present in table" % id)
		return false
	if new_item in table_res.items:
		push_warning("[Registry] override('loot', '%s'): new item is already in the table; would duplicate" % id)
		return false
	table_res.items[idx] = new_item
	# Update the table's _id_index. Override key is the displaced (vanilla)
	# item's file. After this, lib.has("items", old.file) routes to new_item.
	var slot_key: String = String(old_item.get("file") if old_item.get("file") != null else "")
	if slot_key != "" and table_res.has_method("_index_set"):
		table_res._index_set(slot_key, new_item)
	ov[id] = {
		"item": new_item,
		"table": data["table"],
		"table_res": table_res,
		"replaced": old_item,
		"index": idx,
		"slot_key": slot_key,
	}
	_registry_overridden["loot"] = ov
	var reg: Dictionary = _registry_registered.get("loot", {})
	reg[id] = {"item": new_item, "table": data["table"], "table_res": table_res}
	_registry_registered["loot"] = reg
	return true


func _remove_loot(id: String) -> bool:
	var reg: Dictionary = _registry_registered.get("loot", {})
	if not reg.has(id):
		push_warning("[Registry] remove('loot', '%s'): not a mod loot registration" % id)
		return false
	var ov: Dictionary = _registry_overridden.get("loot", {})
	if ov.has(id):
		push_warning("[Registry] remove('loot', '%s'): this id is an override, use revert instead" % id)
		return false
	var entry: Dictionary = reg[id]
	var table_res: Resource = entry["table_res"]
	var item: Resource = entry["item"]
	var idx: int = table_res.items.find(item)
	if idx >= 0:
		table_res.items.remove_at(idx)
	else:
		# Entry was registered but something stripped it externally; clean
		# tracking anyway so subsequent calls are consistent.
		push_warning("[Registry] remove('loot', '%s'): item not found in table; tracking cleared" % id)
	# Drop from the table's _id_index too.
	var item_id: String = String(item.get("file") if item.get("file") != null else "")
	if item_id != "" and table_res.has_method("_index_remove"):
		table_res._index_remove(item_id)
	reg.erase(id)
	_registry_registered["loot"] = reg
	return true


func _revert_loot(id: String) -> bool:
	var ov: Dictionary = _registry_overridden.get("loot", {})
	if not ov.has(id):
		push_warning("[Registry] revert('loot', '%s'): no override to revert" % id)
		return false
	var entry: Dictionary = ov[id]
	var table_res: Resource = entry["table_res"]
	var current_item: Resource = entry["item"]
	var old_item: Resource = entry["replaced"]
	# Find the override in the table now (index may have shifted) and put
	# the original back. If it's gone (some sanitizer removed it), append
	# the original at end so vanilla content isn't lost outright.
	var idx: int = table_res.items.find(current_item)
	if idx >= 0:
		table_res.items[idx] = old_item
	else:
		push_warning("[Registry] revert('loot', '%s'): override entry missing from table, appending original at end" % id)
		table_res.items.append(old_item)
	# Restore the table's _id_index slot to the displaced original.
	var slot_key: String = entry.get("slot_key", "")
	if slot_key != "" and table_res.has_method("_index_set"):
		table_res._index_set(slot_key, old_item)
	ov.erase(id)
	_registry_overridden["loot"] = ov
	var reg: Dictionary = _registry_registered.get("loot", {})
	reg.erase(id)
	_registry_registered["loot"] = reg
	return true
