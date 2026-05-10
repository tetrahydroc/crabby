# ---- recipes kind ----
#
# Recipes.tres has seven `Array[RecipeData]` category fields:
# consumables, medical, equipment, weapons, electronics, misc,
# furniture. Interface const-preloads it then walks the matching
# category at crafting-tab open time. Same timing as loot/events:
# mods must register during their own _ready() before Interface._ready.
#
# patch + revert accept either a String handle or a direct RecipeData
# Resource ref (vanilla recipes patchable without first registering a
# handle). Vanilla ships Equipment + Misc tabs disabled; first mod to
# add a recipe in those categories auto-unlocks the tab via
# scene_nodes.

const _RECIPE_CATEGORIES := ["consumables", "medical", "equipment", "weapons", "electronics", "misc", "furniture"]
const _RECIPES_PATH := "res://Crafting/Recipes.tres"

var _recipes_cache: Resource = null
var _recipes_warned: bool = false


func _recipes_resource() -> Resource:
	if _recipes_cache != null:
		return _recipes_cache
	var res: Resource = load(_RECIPES_PATH)
	if res == null:
		if not _recipes_warned:
			push_warning("[Registry] recipes: Recipes.tres missing at %s; recipes registry is inert" % _RECIPES_PATH)
			_recipes_warned = true
		return null
	_recipes_cache = res
	return res


func _looks_like_recipe_data(res: Resource) -> bool:
	return _resource_has_property(res, "name") \
			and _resource_has_property(res, "input") \
			and _resource_has_property(res, "output")


func _valid_recipe_category(category: String) -> bool:
	return category in _RECIPE_CATEGORIES


func _validate_recipe_data(id: String, verb: String, data: Variant) -> Array:
	if not (data is Dictionary):
		push_warning("[Registry] %s('recipes', '%s', ...) expects Dictionary {recipe, category}, got %s" % [verb, id, typeof(data)])
		return [null, null, ""]
	var d: Dictionary = data
	if not d.has("recipe") or not d.has("category"):
		push_warning("[Registry] %s('recipes', '%s', ...) data dict missing 'recipe' or 'category' key" % [verb, id])
		return [null, null, ""]
	var recipe: Variant = d["recipe"]
	if not (recipe is Resource) or not _looks_like_recipe_data(recipe):
		push_warning("[Registry] %s('recipes', '%s'): recipe is not a RecipeData Resource" % [verb, id])
		return [null, null, ""]
	var category: Variant = d["category"]
	if not (category is String) or not _valid_recipe_category(category):
		push_warning("[Registry] %s('recipes', '%s'): category must be one of %s, got '%s'" \
				% [verb, id, _RECIPE_CATEGORIES, category])
		return [null, null, ""]
	var recipes: Resource = _recipes_resource()
	if recipes == null:
		return [null, null, ""]
	var arr: Variant = recipes.get(category)
	if not (arr is Array):
		push_warning("[Registry] %s('recipes', '%s'): Recipes.%s is not an Array" % [verb, id, category])
		return [null, null, ""]
	return [recipe, arr, String(category)]


func _register_recipe(id: String, data: Variant) -> bool:
	if _mod_added_has("recipes", id):
		push_warning("[Registry] register('recipes', '%s'): already registered (pick a unique handle)" % id)
		return false
	var parts: Array = _validate_recipe_data(id, "register", data)
	var recipe: Variant = parts[0]
	var arr: Variant = parts[1]
	var category: String = parts[2]
	if recipe == null or arr == null:
		return false
	if not _typed_array_accepts(arr, recipe):
		push_warning("[Registry] register('recipes', '%s'): recipe type doesn't match Recipes.%s typed array" % [id, category])
		return false
	if recipe in arr:
		push_warning("[Registry] register('recipes', '%s'): recipe is already present in category '%s'; use override instead" % [id, category])
		return false
	arr.append(recipe)
	# Vanilla `Recipes.tres._id_index` (injected by recipes_index_transform)
	# is the canonical store. Mod-registered recipes don't have a
	# resource_path, so the mod-supplied `id` is the index key.
	var recipes: Resource = _recipes_resource()
	if recipes != null and recipes.has_method("_index_add"):
		recipes._index_add(id, recipe, category)
	# Lightweight mod-added Set for include_vanilla=false filtering.
	_mod_added_mark("recipes", id)
	_unlock_crafting_category_button_if_needed(category)
	return true


## Vanilla ships Equipment + Misc tabs disabled (Interface.tscn) because
## those category arrays are empty. Any mod adding a recipe to either
## category needs the tab clickable; auto-patch via scene_nodes.
const _LOCKED_CATEGORY_BUTTONS := {
	"equipment": "Tools/Crafting/Types/Margin/Buttons/Equipment",
	"misc": "Tools/Crafting/Types/Margin/Buttons/Misc",
}
const _INTERFACE_SCENE_PATH := "res://UI/Interface.tscn"


func _unlock_crafting_category_button_if_needed(category: String) -> void:
	if not _LOCKED_CATEGORY_BUTTONS.has(category):
		return
	var node_path: String = _LOCKED_CATEGORY_BUTTONS[category]
	var snid: String = "%s#%s" % [_INTERFACE_SCENE_PATH, node_path]
	# Restore button to clickable + full alpha. Idempotent: repeat calls
	# from additional mods just re-set the same props. Icons ship at
	# 0.5 alpha as siblings; leave them faded to match the rest.
	_patch_scene_node(snid, {
		"disabled": false,
		"modulate": Color(1.0, 1.0, 1.0, 1.0),
	})


func _override_recipe(id: String, data: Variant) -> bool:
	var ov: Dictionary = _registry_overridden.get("recipes", {})
	if ov.has(id):
		push_warning("[Registry] override('recipes', '%s'): already overridden (revert first to re-override)" % id)
		return false
	if not (data is Dictionary) or not data.has("replaces"):
		push_warning("[Registry] override('recipes', '%s', ...) requires {recipe, category, replaces: RecipeData}" % id)
		return false
	var parts: Array = _validate_recipe_data(id, "override", data)
	var new_recipe: Variant = parts[0]
	var arr: Variant = parts[1]
	var category: String = parts[2]
	if new_recipe == null or arr == null:
		return false
	var old_recipe: Variant = data["replaces"]
	if not (old_recipe is Resource) or not _looks_like_recipe_data(old_recipe):
		push_warning("[Registry] override('recipes', '%s'): 'replaces' is not a RecipeData Resource" % id)
		return false
	if not _typed_array_accepts(arr, new_recipe):
		push_warning("[Registry] override('recipes', '%s'): recipe type doesn't match Recipes.%s typed array" % [id, category])
		return false
	var idx: int = arr.find(old_recipe)
	if idx < 0:
		push_warning("[Registry] override('recipes', '%s'): 'replaces' not present in category '%s'" % [id, category])
		return false
	if new_recipe in arr:
		push_warning("[Registry] override('recipes', '%s'): new recipe already in category; would duplicate" % id)
		return false
	arr[idx] = new_recipe
	# Update vanilla `Recipes.tres._id_index` so reads see the override.
	# The override key is the file-stem of the displaced (vanilla) recipe;
	# that's what's already in the index.
	var slot_key: String = ""
	var recipes2: Resource = _recipes_resource()
	if recipes2 != null:
		if old_recipe.resource_path != "":
			slot_key = old_recipe.resource_path.get_file().get_basename()
		if slot_key != "" and recipes2.has_method("_index_set"):
			recipes2._index_set(slot_key, new_recipe, category)
	ov[id] = {
		"recipe": new_recipe,
		"category": category,
		"replaced": old_recipe,
		"index": idx,
		"slot_key": slot_key,
	}
	_registry_overridden["recipes"] = ov
	# `id` is the mod's tracking handle for the override; `slot_key` is
	# what's in the vanilla index. Both can be queried; mark the handle
	# so include_vanilla=false catches it.
	_mod_added_mark("recipes", id)
	return true


func _resolve_recipe_patch_target(id: Variant) -> Array:
	if id is String:
		# Look up the handle in vanilla's _id_index. Both mod-added
		# recipes (handle-keyed) and vanilla recipes (file-stem keyed)
		# resolve here. Patch on a vanilla recipe by file-stem is a
		# legitimate use case.
		var recipes: Resource = _recipes_resource()
		if recipes != null and "_id_index" in recipes:
			var entry: Variant = recipes._id_index.get(id)
			if entry is Dictionary and entry.has("recipe"):
				return [entry["recipe"], id]
		push_warning("[Registry] patch('recipes', '%s'): no recipe with that id (vanilla file-stem or mod handle)" % id)
		return [null, null]
	if id is Resource and _looks_like_recipe_data(id):
		return [id, "ref:%d" % id.get_instance_id()]
	push_warning("[Registry] patch('recipes', ...): id must be a String handle or a RecipeData Resource")
	return [null, null]


func _patch_recipe(id: Variant, fields: Dictionary) -> bool:
	if fields.is_empty():
		push_warning("[Registry] patch('recipes', ...): empty fields dict is a no-op")
		return false
	var resolved: Array = _resolve_recipe_patch_target(id)
	var target: Variant = resolved[0]
	var key: Variant = resolved[1]
	if target == null:
		return false
	var patched: Dictionary = _registry_patched.get("recipes", {})
	var stash: Dictionary = patched.get(key, {})
	for field in fields.keys():
		var fname := String(field)
		if not _resource_has_property(target, fname):
			push_warning("[Registry] patch('recipes'): field '%s' doesn't exist on RecipeData" % fname)
			continue
		if not stash.has(fname):
			stash[fname] = target.get(fname)
		target.set(fname, fields[field])
	patched[key] = stash
	_registry_patched["recipes"] = patched
	return true


func _remove_recipe(id: String) -> bool:
	if not _mod_added_has("recipes", id):
		push_warning("[Registry] remove('recipes', '%s'): not a mod recipe registration" % id)
		return false
	var ov: Dictionary = _registry_overridden.get("recipes", {})
	if ov.has(id):
		push_warning("[Registry] remove('recipes', '%s'): entry is an override, use revert instead" % id)
		return false
	# Resolve via vanilla index; mod-added recipes live under their handle.
	var recipes: Resource = _recipes_resource()
	if recipes == null or not ("_id_index" in recipes):
		_mod_added_unmark("recipes", id)
		return false
	var entry: Variant = recipes._id_index.get(id)
	if not (entry is Dictionary):
		_mod_added_unmark("recipes", id)
		return false
	var category: String = String(entry.get("category", ""))
	var recipe: Resource = entry.get("recipe")
	var arr: Variant = recipes.get(category)
	if arr is Array and recipe != null:
		var idx: int = arr.find(recipe)
		if idx >= 0:
			arr.remove_at(idx)
		else:
			push_warning("[Registry] remove('recipes', '%s'): recipe not found in %s; tracking cleared" % [id, category])
	if recipes.has_method("_index_remove"):
		recipes._index_remove(id)
	_mod_added_unmark("recipes", id)
	return true


func _revert_recipe(id: Variant, fields: Array) -> bool:
	var did_something := false
	var ov: Dictionary = _registry_overridden.get("recipes", {})
	var patched: Dictionary = _registry_patched.get("recipes", {})
	var patch_key: Variant = null
	var patch_target: Resource = null
	if id is String:
		patch_key = id
		# Resolve via vanilla index (handles both mod handles and
		# vanilla file-stems).
		var recipes_for_target: Resource = _recipes_resource()
		if recipes_for_target != null and "_id_index" in recipes_for_target:
			var entry_t: Variant = recipes_for_target._id_index.get(id)
			if entry_t is Dictionary and entry_t.has("recipe"):
				patch_target = entry_t["recipe"]
	elif id is Resource and _looks_like_recipe_data(id):
		patch_key = "ref:%d" % id.get_instance_id()
		patch_target = id
	if fields.is_empty():
		if patch_key != null and patched.has(patch_key):
			if patch_target != null:
				var stash: Dictionary = patched[patch_key]
				for fname in stash.keys():
					patch_target.set(fname, stash[fname])
			patched.erase(patch_key)
			_registry_patched["recipes"] = patched
			did_something = true
		if id is String and ov.has(id):
			var entry: Dictionary = ov[id]
			var recipes: Resource = _recipes_resource()
			if recipes != null:
				var arr: Variant = recipes.get(entry["category"])
				if arr is Array:
					var current_idx: int = arr.find(entry["recipe"])
					if current_idx >= 0:
						arr[current_idx] = entry["replaced"]
					else:
						push_warning("[Registry] revert('recipes', '%s'): override's recipe missing from %s, appending original at end" % [id, entry["category"]])
						arr.append(entry["replaced"])
				# Restore the vanilla-side index slot to point at the
				# displaced original (same key the override took over).
				var slot_key: String = entry.get("slot_key", "")
				if slot_key != "" and recipes.has_method("_index_set"):
					recipes._index_set(slot_key, entry["replaced"], entry["category"])
			ov.erase(id)
			_registry_overridden["recipes"] = ov
			_mod_added_unmark("recipes", id)
			did_something = true
		if not did_something:
			push_warning("[Registry] revert('recipes'): nothing to revert for that id")
		return did_something
	if patch_key == null or not patched.has(patch_key):
		push_warning("[Registry] revert('recipes'): no patches found for that id")
		return false
	if patch_target == null:
		push_warning("[Registry] revert('recipes'): patch target no longer resolves")
		return false
	var stash2: Dictionary = patched[patch_key]
	for field in fields:
		var fname := String(field)
		if not stash2.has(fname):
			push_warning("[Registry] revert('recipes'): field '%s' wasn't patched" % fname)
			continue
		patch_target.set(fname, stash2[fname])
		stash2.erase(fname)
		did_something = true
	if stash2.is_empty():
		patched.erase(patch_key)
	else:
		patched[patch_key] = stash2
	_registry_patched["recipes"] = patched
	return did_something


# ---- array-ops on recipe Resource fields ----

func _append_recipe(id: Variant, field: String, values: Array, allow_duplicates: bool) -> bool:
	var resolved: Array = _resolve_recipe_patch_target(id)
	var target: Variant = resolved[0]
	var key: Variant = resolved[1]
	if target == null:
		return false
	return _array_op_on_resource("recipes", key, target, field, "append", values, allow_duplicates)


func _prepend_recipe(id: Variant, field: String, values: Array, allow_duplicates: bool) -> bool:
	var resolved: Array = _resolve_recipe_patch_target(id)
	var target: Variant = resolved[0]
	var key: Variant = resolved[1]
	if target == null:
		return false
	return _array_op_on_resource("recipes", key, target, field, "prepend", values, allow_duplicates)


func _remove_from_recipe(id: Variant, field: String, values: Array) -> bool:
	var resolved: Array = _resolve_recipe_patch_target(id)
	var target: Variant = resolved[0]
	var key: Variant = resolved[1]
	if target == null:
		return false
	return _array_op_on_resource("recipes", key, target, field, "remove_from", values, false)
