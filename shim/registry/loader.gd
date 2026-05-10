# ---- loader-based kinds: scene_paths, shelters, random_scenes ----
#
# All three mutate state on the Loader autoload. The rewriter
# (loader_transform.rs) injects:
#   _rtv_mod_scene_paths       - register() writes here
#   _rtv_override_scene_paths  - override() writes here
#   _rtv_vanilla_shelters      - snapshot of the original const for revert
#   `var shelters`             - original `const shelters` rewritten
#                                 mutable so the shelters registry can append
#   prelude in LoadScene + a `_rtv_resolve_scene_path()` helper that
#   consults the override/mod dicts before the vanilla if-elif chain
#
# Timing: Loader is an autoload, so the prelude is active from boot.
# Mod registrations land any time after the Loader autoload is in
# the tree (mod _ready() is safe).
#
# Schemas:
#   scene_paths.register/override:
#     {path: String, menu?: bool, shelter?: bool, permadeath?: bool, tutorial?: bool}
#   shelters.register: {path?: String, menu?: bool, ...}
#     If `path` is provided, auto-registers a paired scene_paths entry
#     so the shelter name resolves to a real scene.
#   random_scenes.register: {path: String}

func _loader_node() -> Node:
	var ldr: Node = get_tree().root.get_node_or_null("Loader")
	if ldr == null:
		push_warning("[Registry] Loader autoload not in tree yet; is the loader still booting?")
	return ldr


# Lazy cache of vanilla scene-name consts on Loader.gd. Populated on
# first lookup via the Script's get_script_constant_map().
var _vanilla_scene_const_cache: Dictionary = {}
var _vanilla_scene_const_built: bool = false


func _vanilla_scene_const_exists(ldr: Node, id: String) -> bool:
	if not _vanilla_scene_const_built:
		var script: Script = ldr.get_script()
		if script != null:
			_vanilla_scene_const_cache = script.get_script_constant_map()
		_vanilla_scene_const_built = true
	return _vanilla_scene_const_cache.has(id)


# -------- scene_paths --------

func _register_scene_path(id: String, data: Variant) -> bool:
	if not (data is Dictionary):
		push_warning("[Registry] register('scene_paths', '%s', ...) expects Dictionary, got %s" % [id, typeof(data)])
		return false
	var d: Dictionary = data
	if not d.has("path") or not (d["path"] is String):
		push_warning("[Registry] register('scene_paths', '%s'): data requires string 'path' key" % id)
		return false
	var ldr: Node = _loader_node()
	if ldr == null:
		return false
	if not ("_rtv_mod_scene_paths" in ldr):
		push_warning("[Registry] register('scene_paths'): Loader.gd is missing injected scene-path fields; rewriter didn't fire")
		return false
	if ldr._rtv_mod_scene_paths.has(id) or ldr._rtv_override_scene_paths.has(id):
		push_warning("[Registry] register('scene_paths', '%s'): already registered/overridden by a mod" % id)
		return false
	if _vanilla_scene_const_exists(ldr, id):
		push_warning("[Registry] register('scene_paths', '%s'): name collides with a vanilla scene const; use override instead" % id)
		return false
	ldr._rtv_mod_scene_paths[id] = d
	var reg: Dictionary = _registry_registered.get("scene_paths", {})
	reg[id] = d
	_registry_registered["scene_paths"] = reg
	return true


func _override_scene_path(id: String, data: Variant) -> bool:
	if not (data is Dictionary):
		push_warning("[Registry] override('scene_paths', '%s', ...) expects Dictionary, got %s" % [id, typeof(data)])
		return false
	var d: Dictionary = data
	if not d.has("path") or not (d["path"] is String):
		push_warning("[Registry] override('scene_paths', '%s'): data requires string 'path' key" % id)
		return false
	var ldr: Node = _loader_node()
	if ldr == null:
		return false
	if not ("_rtv_override_scene_paths" in ldr):
		push_warning("[Registry] override('scene_paths'): Loader.gd is missing injected fields")
		return false
	var is_vanilla_const: bool = _vanilla_scene_const_exists(ldr, id)
	var is_mod_registration: bool = ldr._rtv_mod_scene_paths.has(id)
	if not is_vanilla_const and not is_mod_registration:
		push_warning("[Registry] override('scene_paths', '%s'): no vanilla scene const or mod registration with that name" % id)
		return false
	var ov: Dictionary = _registry_overridden.get("scene_paths", {})
	if not ov.has(id):
		# Stash for revert. For vanilla, the if-elif branches' flags
		# aren't replicated here; revert just clears the override and
		# vanilla's own if-elif handles the restore.
		if is_vanilla_const:
			ov[id] = {"vanilla": true}
		else:
			ov[id] = {"vanilla": false, "data": ldr._rtv_mod_scene_paths[id]}
		_registry_overridden["scene_paths"] = ov
	ldr._rtv_override_scene_paths[id] = d
	return true


func _patch_scene_path(id: String, fields: Dictionary) -> bool:
	if fields.is_empty():
		push_warning("[Registry] patch('scene_paths', '%s'): empty fields is a no-op" % id)
		return false
	var ldr: Node = _loader_node()
	if ldr == null:
		return false
	# Patch operates on the mod's dict entry. Walk override first, then
	# mod registration. Vanilla consts aren't patchable here; mods
	# should override() instead.
	var target_dict: Dictionary
	var target_store: String  # "override" or "mod"
	if ldr._rtv_override_scene_paths.has(id):
		target_dict = ldr._rtv_override_scene_paths[id]
		target_store = "override"
	elif ldr._rtv_mod_scene_paths.has(id):
		target_dict = ldr._rtv_mod_scene_paths[id]
		target_store = "mod"
	else:
		push_warning("[Registry] patch('scene_paths', '%s'): no mod registration or override to patch" % id)
		return false
	var patched: Dictionary = _registry_patched.get("scene_paths", {})
	var stash: Dictionary = patched.get(id, {})
	for field in fields.keys():
		var fname := String(field)
		if not stash.has(fname):
			# Sentinel records "key didn't exist" so revert can erase
			# vs restore accurately.
			if target_dict.has(fname):
				stash[fname] = target_dict[fname]
			else:
				stash[fname] = "__rtv_missing__"
		target_dict[fname] = fields[field]
	if target_store == "override":
		ldr._rtv_override_scene_paths[id] = target_dict
	else:
		ldr._rtv_mod_scene_paths[id] = target_dict
		var reg: Dictionary = _registry_registered.get("scene_paths", {})
		reg[id] = target_dict
		_registry_registered["scene_paths"] = reg
	patched[id] = stash
	_registry_patched["scene_paths"] = patched
	return true


func _remove_scene_path(id: String) -> bool:
	var ldr: Node = _loader_node()
	if ldr == null:
		return false
	var reg: Dictionary = _registry_registered.get("scene_paths", {})
	if not reg.has(id):
		push_warning("[Registry] remove('scene_paths', '%s'): not a mod registration" % id)
		return false
	var ov: Dictionary = _registry_overridden.get("scene_paths", {})
	if ov.has(id):
		push_warning("[Registry] remove('scene_paths', '%s'): entry is an override, use revert instead" % id)
		return false
	ldr._rtv_mod_scene_paths.erase(id)
	reg.erase(id)
	_registry_registered["scene_paths"] = reg
	return true


func _revert_scene_path(id: String, fields: Array) -> bool:
	var ldr: Node = _loader_node()
	if ldr == null:
		return false
	var did_something := false
	var ov: Dictionary = _registry_overridden.get("scene_paths", {})
	var patched: Dictionary = _registry_patched.get("scene_paths", {})
	var target_dict: Dictionary
	var stash: Dictionary
	if fields.is_empty():
		# Patches first (onto whatever dict is current).
		if patched.has(id):
			stash = patched[id]
			if ldr._rtv_override_scene_paths.has(id):
				target_dict = ldr._rtv_override_scene_paths[id]
			elif ldr._rtv_mod_scene_paths.has(id):
				target_dict = ldr._rtv_mod_scene_paths[id]
			for fname in stash.keys():
				var stashed_val: Variant = stash[fname]
				if stashed_val is String and stashed_val == "__rtv_missing__":
					target_dict.erase(fname)
				else:
					target_dict[fname] = stashed_val
			patched.erase(id)
			_registry_patched["scene_paths"] = patched
			did_something = true
		if ov.has(id):
			ldr._rtv_override_scene_paths.erase(id)
			ov.erase(id)
			_registry_overridden["scene_paths"] = ov
			did_something = true
		if not did_something:
			push_warning("[Registry] revert('scene_paths', '%s'): nothing to revert" % id)
		return did_something
	# Per-field patch revert.
	if not patched.has(id):
		push_warning("[Registry] revert('scene_paths', '%s', %s): no patches on this id" % [id, fields])
		return false
	if ldr._rtv_override_scene_paths.has(id):
		target_dict = ldr._rtv_override_scene_paths[id]
	elif ldr._rtv_mod_scene_paths.has(id):
		target_dict = ldr._rtv_mod_scene_paths[id]
	else:
		push_warning("[Registry] revert('scene_paths', '%s'): id no longer resolves" % id)
		return false
	stash = patched[id]
	for field in fields:
		var fname := String(field)
		if not stash.has(fname):
			push_warning("[Registry] revert('scene_paths', '%s'): field '%s' wasn't patched" % [id, fname])
			continue
		var stashed_val: Variant = stash[fname]
		if stashed_val is String and stashed_val == "__rtv_missing__":
			target_dict.erase(fname)
		else:
			target_dict[fname] = stashed_val
		stash.erase(fname)
		did_something = true
	if stash.is_empty():
		patched.erase(id)
	else:
		patched[id] = stash
	_registry_patched["scene_paths"] = patched
	return did_something


# -------- shelters / maps --------
#
# The shelters list (vanilla `Loader.shelters` made mutable by the
# loader rewriter) holds bare strings. Registering a shelter or map
# also requires a scene behind the name; if `path` is provided, a
# paired scene_paths entry is auto-registered so LoadScene(name) works.
# Otherwise the name is assumed already resolvable (vanilla or
# pre-registered).
#
# `shelters` and `maps` share storage and most behavior. The only
# difference is the default `shelter` flag (true vs false), which
# controls TWO things:
#   1. gameData.shelter, set during LoadScene's prelude
#   2. Loader.LoadShelter(name), called from Compiler.Spawn for
#      shelters; loads/saves per-shelter persistent state. Maps don't.
#
# Full registration schema (all optional except `path` for newly-added
# scenes):
#   path              - res:// path to the .tscn (auto-registers scene_paths)
#   transition_text   - override for the "Loading X..." label (defaults to id)
#   exit_spawn        - transition node name to spawn at on arrival
#                       (e.g. "Door_Apartment_Exit")
#   entrance_spawn    - transition node name in `connected_to` to spawn
#                       at when leaving this shelter back to its parent map
#   connected_to      - vanilla map name where this shelter's entrance lives
#   connected_content - Array of {path, position, rotation} entries
#                       spawned into /root/Map/Content when player enters
#                       connected_to
#   shelter           - bool, default true for shelters / false for maps
#
# Mirrors B_Loader's add_shelter/add_map dict shape; existing
# B_Loader-pattern mods migrate by changing one call site (or via the
# compat shim methods on Loader.gd, which translate to the same path).

func _register_shelter(id: String, data: Variant) -> bool:
	return _register_shelter_or_map(id, data, true, "shelters")


func _register_map(id: String, data: Variant) -> bool:
	return _register_shelter_or_map(id, data, false, "maps")


func _register_shelter_or_map(id: String, data: Variant, default_shelter: bool, label: String) -> bool:
	var ldr: Node = _loader_node()
	if ldr == null:
		return false
	if not (data is Dictionary):
		push_warning("[Registry] register('%s', '%s', ...) expects Dictionary (can be empty if scene already registered)" % [label, id])
		return false
	var d: Dictionary = data
	# Shelters and maps share the _registry_registered['shelters'] bucket
	# so cross-surface collisions fail loud; a name can only resolve to
	# one entry in Compiler.Spawn.
	var reg: Dictionary = _registry_registered.get("shelters", {})
	if reg.has(id):
		push_warning("[Registry] register('%s', '%s'): already registered as shelter or map" % [label, id])
		return false
	if id in ldr.shelters:
		push_warning("[Registry] register('%s', '%s'): name already in shelters list (vanilla?)" % [label, id])
		return false
	# Build the rich entry consumed by Compiler.Spawn's prelude.
	var is_shelter: bool = bool(d.get("shelter", default_shelter))
	var entry: Dictionary = {
		"shelter": is_shelter,
		"transition_text": String(d.get("transition_text", id)),
		"exit_spawn": String(d.get("exit_spawn", "")),
		"entrance_spawn": String(d.get("entrance_spawn", "")),
		"connected_to": String(d.get("connected_to", "")),
		"connected_content": d.get("connected_content", []),
	}
	# If a `path` is given, auto-register the scene_paths entry so the
	# name resolves to a real scene. Forward the shelter flag (LoadScene
	# prelude reads `shelter` from the scene_paths entry to set
	# gameData.shelter) and the transition_text so the loading-screen
	# label uses the modder's preferred name.
	var auto_scene_path := false
	if d.has("path"):
		var sp_data: Dictionary = {}
		sp_data["path"] = d["path"]
		sp_data["shelter"] = is_shelter
		if d.has("menu"): sp_data["menu"] = d["menu"]
		if d.has("permadeath"): sp_data["permadeath"] = d["permadeath"]
		if d.has("tutorial"): sp_data["tutorial"] = d["tutorial"]
		sp_data["transition_text"] = entry["transition_text"]
		if not _register_scene_path(id, sp_data):
			return false
		auto_scene_path = true
	ldr.shelters.append(id)
	# Persist the rich entry into the rewriter-injected dict so
	# Compiler.Spawn's prelude can consult it.
	if "_rtv_mod_shelters" in ldr:
		ldr._rtv_mod_shelters[id] = entry
	else:
		push_warning("[Registry] register('%s', '%s'): Loader is missing _rtv_mod_shelters; rewriter didn't fire" % [label, id])
	reg[id] = {"auto_scene_path": auto_scene_path, "entry": entry, "kind": label}
	_registry_registered["shelters"] = reg
	return true


func _remove_shelter(id: String) -> bool:
	return _remove_shelter_or_map(id, "shelters")


func _remove_map(id: String) -> bool:
	return _remove_shelter_or_map(id, "maps")


func _remove_shelter_or_map(id: String, label: String) -> bool:
	var ldr: Node = _loader_node()
	if ldr == null:
		return false
	var reg: Dictionary = _registry_registered.get("shelters", {})
	if not reg.has(id):
		push_warning("[Registry] remove('%s', '%s'): not a mod registration" % [label, id])
		return false
	var meta: Dictionary = reg[id]
	# Cross-surface remove guard: don't let `remove('maps', X)` succeed
	# if X was registered as a shelter (or vice versa). Loud is better
	# than a silent no-op or the wrong cleanup path.
	if meta.get("kind", "shelters") != label:
		push_warning("[Registry] remove('%s', '%s'): id was registered as '%s', use that registry to remove" \
				% [label, id, meta.get("kind", "shelters")])
		return false
	var idx: int = ldr.shelters.find(id)
	if idx >= 0:
		ldr.shelters.remove_at(idx)
	if meta.get("auto_scene_path", false):
		_remove_scene_path(id)
	if "_rtv_mod_shelters" in ldr:
		ldr._rtv_mod_shelters.erase(id)
	reg.erase(id)
	_registry_registered["shelters"] = reg
	return true


# -------- random_scenes --------
#
# `randomScenes` is already a `var` in vanilla Loader.gd; no rewriter
# changes needed. Just append/remove paths.

func _register_random_scene(id: String, data: Variant) -> bool:
	var ldr: Node = _loader_node()
	if ldr == null:
		return false
	if not (data is Dictionary) or not data.has("path") or not (data["path"] is String):
		push_warning("[Registry] register('random_scenes', '%s', ...) expects Dictionary with 'path' key" % id)
		return false
	var reg: Dictionary = _registry_registered.get("random_scenes", {})
	if reg.has(id):
		push_warning("[Registry] register('random_scenes', '%s'): already registered" % id)
		return false
	var path: String = data["path"]
	if path in ldr.randomScenes:
		push_warning("[Registry] register('random_scenes', '%s'): path already in randomScenes" % id)
		return false
	ldr.randomScenes.append(path)
	reg[id] = {"path": path}
	_registry_registered["random_scenes"] = reg
	return true


func _remove_random_scene(id: String) -> bool:
	var ldr: Node = _loader_node()
	if ldr == null:
		return false
	var reg: Dictionary = _registry_registered.get("random_scenes", {})
	if not reg.has(id):
		push_warning("[Registry] remove('random_scenes', '%s'): not a mod registration" % id)
		return false
	var path: String = reg[id]["path"]
	var idx: int = ldr.randomScenes.find(path)
	if idx >= 0:
		ldr.randomScenes.remove_at(idx)
	reg.erase(id)
	_registry_registered["random_scenes"] = reg
	return true
