# ---- fish_species kind ----
#
# Vanilla FishPool.gd is a per-scene MeshInstance3D with @export var
# species: Array[PackedScene] populated in the editor. Its _ready()
# picks 1-10 random fish from species and instantiates them.
#
# The rewriter (fish_pool_transform.rs) injects a prelude at the top
# of FishPool._ready() that reads Engine.get_meta("_rtv_fish_species",
# []) and merges matching entries into the local `species` array
# before the random-spawn loop.
#
# Schema:
#   register: {scene: PackedScene, pool_id: String}
#     pool_id "all" (default) targets every pool; explicit names like
#     "FP_2" target the matching node by Node.name.
#
# Verbs: register, remove. Override/patch not meaningful for a flat
# list of {scene, pool_id} tuples.
#
# Timing: FishPool is a scene Node, not an autoload. Its _ready()
# fires when its containing scene loads. Mods must register before
# entering the map scene; mod autoload _ready() is fine since the main
# menu loads first.

const _FISH_ENGINE_META_KEY := "_rtv_fish_species"


## Flatten id registrations into a flat Array for the prelude loop.
## Registration-order preservation keeps behavior deterministic across
## mod load orders.
func _rebuild_fish_engine_meta() -> void:
	var flat: Array = []
	var reg: Dictionary = _registry_registered.get("fish_species", {})
	for id in reg.keys():
		flat.append(reg[id])
	Engine.set_meta(_FISH_ENGINE_META_KEY, flat)


func _register_fish_species(id: String, data: Variant) -> bool:
	var reg: Dictionary = _registry_registered.get("fish_species", {})
	if reg.has(id):
		push_warning("[Registry] register('fish_species', '%s'): already registered (pick a unique handle)" % id)
		return false
	if not (data is Dictionary):
		push_warning("[Registry] register('fish_species', '%s', ...) expects Dictionary {scene, pool_id}, got %s" % [id, typeof(data)])
		return false
	var d: Dictionary = data
	if not d.has("scene"):
		push_warning("[Registry] register('fish_species', '%s'): data missing 'scene' key" % id)
		return false
	var scene: Variant = d["scene"]
	if not (scene is PackedScene):
		push_warning("[Registry] register('fish_species', '%s'): scene is not a PackedScene" % id)
		return false
	# Default pool_id to "all"; most mods want their fish in every pool.
	var pool_id: String = "all"
	if d.has("pool_id"):
		if not (d["pool_id"] is String):
			push_warning("[Registry] register('fish_species', '%s'): pool_id must be a String" % id)
			return false
		pool_id = d["pool_id"]
	reg[id] = {"scene": scene, "pool_id": pool_id}
	_registry_registered["fish_species"] = reg
	_rebuild_fish_engine_meta()
	return true


func _remove_fish_species(id: String) -> bool:
	var reg: Dictionary = _registry_registered.get("fish_species", {})
	if not reg.has(id):
		push_warning("[Registry] remove('fish_species', '%s'): not registered by a mod" % id)
		return false
	reg.erase(id)
	_registry_registered["fish_species"] = reg
	_rebuild_fish_engine_meta()
	return true
