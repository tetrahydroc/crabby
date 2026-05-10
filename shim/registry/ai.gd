# ---- ai_types kind ----
#
# Vanilla AISpawner.gd hardcodes a zone -> agent-scene mapping in
# _ready(). The rewriter (ai_spawner_transform.rs) wraps each
# `agent = <name>` to call into _rtv_resolve_ai_type, which reads
# Engine.get_meta("_rtv_ai_overrides", {}) for mod-supplied overrides.
#
# AISpawner is a per-scene Node3D (not an autoload); many instances
# may run _ready independently. Engine-meta is the broadcast channel.
# Zones are addressed by String name matching the Zone enum
# ("Area05", "BorderZone", "Vostok"). Resolver does the int -> name
# conversion via Zone.keys()[zone_int].
#
# Schema:
#   register / override: {scene: PackedScene, zone: String}

const _AI_VALID_ZONES := ["Area05", "BorderZone", "Vostok"]
const _AI_ENGINE_META_KEY := "_rtv_ai_overrides"


## Collapse all active id registrations into a single zone -> scene
## dict for the resolver. Last write wins on contested zones.
func _rebuild_ai_engine_meta() -> void:
	var flat: Dictionary = {}
	var reg: Dictionary = _registry_registered.get("ai_types", {})
	for id in reg.keys():
		var entry: Dictionary = reg[id]
		flat[entry["zone"]] = entry["scene"]
	Engine.set_meta(_AI_ENGINE_META_KEY, flat)


func _validate_ai_type_data(id: String, verb: String, data: Variant) -> Array:
	if not (data is Dictionary):
		push_warning("[Registry] %s('ai_types', '%s', ...) expects Dictionary {scene, zone}, got %s" % [verb, id, typeof(data)])
		return [null, ""]
	var d: Dictionary = data
	if not d.has("scene") or not d.has("zone"):
		push_warning("[Registry] %s('ai_types', '%s', ...) data dict missing 'scene' or 'zone' key" % [verb, id])
		return [null, ""]
	var scene: Variant = d["scene"]
	if not (scene is PackedScene):
		push_warning("[Registry] %s('ai_types', '%s'): scene is not a PackedScene" % [verb, id])
		return [null, ""]
	var zone: Variant = d["zone"]
	if not (zone is String):
		push_warning("[Registry] %s('ai_types', '%s'): zone must be a String (e.g. 'Area05')" % [verb, id])
		return [null, ""]
	if not (zone in _AI_VALID_ZONES):
		push_warning("[Registry] %s('ai_types', '%s'): unknown zone '%s' (valid: %s)" % [verb, id, zone, _AI_VALID_ZONES])
		return [null, ""]
	return [scene, zone]


func _register_ai_type(id: String, data: Variant) -> bool:
	var reg: Dictionary = _registry_registered.get("ai_types", {})
	if reg.has(id):
		push_warning("[Registry] register('ai_types', '%s'): already registered (pick a unique handle or use override)" % id)
		return false
	var parts: Array = _validate_ai_type_data(id, "register", data)
	var scene: Variant = parts[0]
	var zone: String = parts[1]
	if scene == null:
		return false
	# Single registration per zone; override forces a swap.
	for existing_id in reg.keys():
		if reg[existing_id]["zone"] == zone:
			push_warning("[Registry] register('ai_types', '%s'): zone '%s' already claimed by '%s'; use override to replace" % [id, zone, existing_id])
			return false
	reg[id] = {"scene": scene, "zone": zone}
	_registry_registered["ai_types"] = reg
	_rebuild_ai_engine_meta()
	return true


func _override_ai_type(id: String, data: Variant) -> bool:
	# Override = "claim this zone even if another mod did." Drops any
	# conflicting registrations and stashes them so revert restores.
	var ov: Dictionary = _registry_overridden.get("ai_types", {})
	if ov.has(id):
		push_warning("[Registry] override('ai_types', '%s'): already overridden (revert first)" % id)
		return false
	var parts: Array = _validate_ai_type_data(id, "override", data)
	var scene: Variant = parts[0]
	var zone: String = parts[1]
	if scene == null:
		return false
	var reg: Dictionary = _registry_registered.get("ai_types", {})
	var displaced: Array = []
	for existing_id in reg.keys():
		if reg[existing_id]["zone"] == zone:
			displaced.append({"id": existing_id, "entry": reg[existing_id]})
	for entry in displaced:
		reg.erase(entry["id"])
	reg[id] = {"scene": scene, "zone": zone}
	_registry_registered["ai_types"] = reg
	ov[id] = {"displaced": displaced, "zone": zone}
	_registry_overridden["ai_types"] = ov
	_rebuild_ai_engine_meta()
	return true


func _remove_ai_type(id: String) -> bool:
	var reg: Dictionary = _registry_registered.get("ai_types", {})
	if not reg.has(id):
		push_warning("[Registry] remove('ai_types', '%s'): not registered by a mod" % id)
		return false
	var ov: Dictionary = _registry_overridden.get("ai_types", {})
	if ov.has(id):
		push_warning("[Registry] remove('ai_types', '%s'): entry is an override, use revert instead" % id)
		return false
	reg.erase(id)
	_registry_registered["ai_types"] = reg
	_rebuild_ai_engine_meta()
	return true


func _revert_ai_type(id: String) -> bool:
	var ov: Dictionary = _registry_overridden.get("ai_types", {})
	if not ov.has(id):
		push_warning("[Registry] revert('ai_types', '%s'): no override to revert" % id)
		return false
	var entry: Dictionary = ov[id]
	var reg: Dictionary = _registry_registered.get("ai_types", {})
	reg.erase(id)
	var displaced: Array = entry["displaced"]
	for d in displaced:
		reg[d["id"]] = d["entry"]
	_registry_registered["ai_types"] = reg
	ov.erase(id)
	_registry_overridden["ai_types"] = ov
	_rebuild_ai_engine_meta()
	return true
