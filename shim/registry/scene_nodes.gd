# ---- scene_nodes kind ----
#
# Patch-only registry for mutating node properties inside vanilla
# scenes without shipping a full-scene override.
#
#   lib.patch(lib.Registry.SCENE_NODES,
#             "res://UI/Interface.tscn#Tools/Crafting/Types/Margin/Buttons/Equipment",
#             {disabled = false, modulate = Color(1,1,1,1)})
#
# id format: "<scene_path>#<node_path>", split on the FIRST '#'.
#
# Mechanism: subscribe to `get_tree().node_added` at frameworks_ready.
# Godot sets `node.scene_file_path` only on instantiated scene roots,
# providing a cheap filter. On match, walk the registered node_paths
# and apply property patches. Fires BEFORE the node's `_ready`, so
# `@onready` values that depend on the patched props observe the
# patched state.

# scene_path -> node_path -> {prop -> value} for forward patches.
var _scene_node_patches: Dictionary = {}
# Parallel stash for revert: same shape, holds the value before the
# first patch on that (scene, node, prop) triple. Populated lazily at
# apply time (not patch() time; no live instance yet).
var _scene_node_stash: Dictionary = {}
var _scene_nodes_listener_connected: bool = false
# Memoize successful probe validations so repeat patch() calls with the
# same id+fields (e.g., recipes auto-unlock once per registered recipe)
# don't re-instantiate the scene N times. Keyed by
# "<scene_path>#<node_path>|<sorted,fields>".
var _validated_patches: Dictionary = {}


## Idempotent. Safe to call before or at frameworks_ready.
func _scene_nodes_connect_listener() -> void:
	if _scene_nodes_listener_connected:
		return
	var tree: SceneTree = get_tree()
	if tree == null:
		push_warning("[Registry] scene_nodes: no SceneTree at connect time; listener disabled")
		return
	tree.node_added.connect(_on_any_node_added)
	_scene_nodes_listener_connected = true


func _on_any_node_added(node: Node) -> void:
	var scene_path: String = node.scene_file_path
	if scene_path.is_empty():
		return
	if not _scene_node_patches.has(scene_path):
		return
	_apply_patches_for_scene_root(scene_path, node)


func _apply_patches_for_scene_root(scene_path: String, scene_root: Node) -> void:
	var per_node: Dictionary = _scene_node_patches[scene_path]
	var stash_per_scene: Dictionary = _scene_node_stash.get(scene_path, {})
	for node_path in per_node.keys():
		var target: Node = _resolve_scene_target(scene_root, node_path)
		if target == null:
			push_warning("[Registry] scene_nodes: node '%s' not found in instantiated '%s'; patch skipped" \
					% [node_path, scene_path])
			continue
		var props: Dictionary = per_node[node_path]
		var stash_per_node: Dictionary = stash_per_scene.get(node_path, {})
		for prop in props.keys():
			var fname: String = String(prop)
			if not _node_has_property(target, fname):
				push_warning("[Registry] scene_nodes: property '%s' not found on node '%s' in '%s'; skipped" \
						% [fname, node_path, scene_path])
				continue
			if not stash_per_node.has(fname):
				stash_per_node[fname] = target.get(fname)
			target.set(fname, props[fname])
		stash_per_scene[node_path] = stash_per_node
	_scene_node_stash[scene_path] = stash_per_scene


## Split 'scene#node' on the FIRST '#'. Returns [scene_path, node_path]
## or [null, null] on malformed input.
##
## Accepts three forms for the node_path side:
##   "...tscn#Foo/Bar"  -> node_path = "Foo/Bar"   (descendant)
##   "...tscn#."        -> node_path = "."         (root, explicit)
##   "...tscn#"         -> node_path = ""          (root, empty form)
## The empty and "." forms both target the scene's root node. Without
## this, patching a property that lives on the scene's root requires
## constructing an artificial child path (which may not exist), since
## get_node_or_null walks DOWN from the root and can't return the root
## itself by name.
func _split_scene_node_id(id: String) -> Array:
	var hash_idx: int = id.find("#")
	if hash_idx <= 0:
		return [null, null]
	var scene_path: String = id.substr(0, hash_idx)
	var node_path: String = id.substr(hash_idx + 1)
	if not scene_path.begins_with("res://"):
		return [null, null]
	return [scene_path, node_path]


# Resolve a node_path against a scene root. Empty or "." means the root
# itself; anything else falls through to get_node_or_null. Returns null
# only when a non-root path doesn't resolve.
func _resolve_scene_target(scene_root: Node, node_path: String) -> Node:
	if node_path == "" or node_path == ".":
		return scene_root
	return scene_root.get_node_or_null(NodePath(node_path))


## Probe-validate at patch time by instantiating + freeing the scene.
## Cold path (mod boot) only. Cached so recipe auto-unlock doesn't
## re-instantiate Interface.tscn per recipe.
func _validate_scene_node_patch(scene_path: String, node_path: String, fields: Dictionary) -> bool:
	var field_keys: Array = []
	for k in fields.keys():
		field_keys.append(String(k))
	field_keys.sort()
	var cache_key: String = "%s#%s|%s" % [scene_path, node_path, ",".join(field_keys)]
	if _validated_patches.has(cache_key):
		return true
	var pscene: Resource = load(scene_path)
	if pscene == null or not (pscene is PackedScene):
		push_warning("[Registry] patch('scene_nodes'): scene '%s' failed to load (not a PackedScene)" % scene_path)
		return false
	var probe: Node = (pscene as PackedScene).instantiate()
	if probe == null:
		push_warning("[Registry] patch('scene_nodes'): scene '%s' failed to instantiate for validation" % scene_path)
		return false
	var target: Node = _resolve_scene_target(probe, node_path)
	if target == null:
		push_warning("[Registry] patch('scene_nodes', '%s#%s'): node path doesn't resolve in scene; check hierarchy" \
				% [scene_path, node_path])
		probe.queue_free()
		return false
	for prop in fields.keys():
		if not _node_has_property(target, String(prop)):
			push_warning("[Registry] patch('scene_nodes', '%s#%s'): property '%s' not found on node (class=%s)" \
					% [scene_path, node_path, prop, target.get_class()])
			probe.queue_free()
			return false
	probe.queue_free()
	_validated_patches[cache_key] = true
	return true


func _node_has_property(node: Node, prop: String) -> bool:
	for p in node.get_property_list():
		if p.get("name") == prop:
			return true
	return false


func _patch_scene_node(id: String, fields: Dictionary) -> bool:
	if fields.is_empty():
		push_warning("[Registry] patch('scene_nodes', '%s'): empty fields dict is a no-op" % id)
		return false
	var parts: Array = _split_scene_node_id(id)
	var scene_path: Variant = parts[0]
	var node_path: Variant = parts[1]
	if scene_path == null:
		push_warning("[Registry] patch('scene_nodes', '%s'): id must be '<res://scene_path>#<node_path>'" % id)
		return false
	if not _validate_scene_node_patch(scene_path, node_path, fields):
		return false
	# Lazy-connect in case patch() runs before frameworks_ready.
	_scene_nodes_connect_listener()
	var per_node: Dictionary = _scene_node_patches.get(scene_path, {})
	var props: Dictionary = per_node.get(node_path, {})
	for prop in fields.keys():
		props[String(prop)] = fields[prop]
	per_node[node_path] = props
	_scene_node_patches[scene_path] = per_node
	# Mirror into _registry_patched for has/get_entry consistency. The
	# stash for revert is populated at apply time, not here (no live
	# instance yet to read original values from).
	var patched: Dictionary = _registry_patched.get("scene_nodes", {})
	var pat_entry: Dictionary = patched.get(id, {})
	for prop in fields.keys():
		pat_entry[String(prop)] = fields[prop]
	patched[id] = pat_entry
	_registry_patched["scene_nodes"] = patched
	# Apply to any live instances of the scene already in the tree.
	_apply_patch_to_live_instances(scene_path)
	return true


func _apply_patch_to_live_instances(scene_path: String) -> void:
	var tree: SceneTree = get_tree()
	if tree == null:
		return
	_walk_for_scene_roots(tree.root, scene_path)


func _walk_for_scene_roots(node: Node, scene_path: String) -> void:
	if node.scene_file_path == scene_path:
		_apply_patches_for_scene_root(scene_path, node)
		# Don't recurse into a matched root; nested instances of the
		# same scene are exceedingly rare and will surface via their
		# own node_added.
		return
	for child in node.get_children():
		_walk_for_scene_roots(child, scene_path)


func _revert_scene_node(id: String, fields: Array) -> bool:
	var parts: Array = _split_scene_node_id(id)
	var scene_path: Variant = parts[0]
	var node_path: Variant = parts[1]
	if scene_path == null:
		push_warning("[Registry] revert('scene_nodes', '%s'): id must be '<res://scene_path>#<node_path>'" % id)
		return false
	var patched: Dictionary = _registry_patched.get("scene_nodes", {})
	if not patched.has(id):
		push_warning("[Registry] revert('scene_nodes', '%s'): nothing patched at that id" % id)
		return false
	var pat_entry: Dictionary = patched[id]
	var per_node: Dictionary = _scene_node_patches.get(scene_path, {})
	var props: Dictionary = per_node.get(node_path, {})
	var stash_per_scene: Dictionary = _scene_node_stash.get(scene_path, {})
	var stash_per_node: Dictionary = stash_per_scene.get(node_path, {})
	var targets: Array[String] = []
	if fields.is_empty():
		for k in pat_entry.keys():
			targets.append(String(k))
	else:
		for k in fields:
			targets.append(String(k))
	# Restore stashed values on every live instance of the scene.
	var live_roots: Array[Node] = []
	var tree: SceneTree = get_tree()
	if tree != null:
		_collect_scene_roots(tree.root, scene_path, live_roots)
	for fname in targets:
		if not stash_per_node.has(fname) and not fields.is_empty():
			# Per-field revert for a field that was never observed on
			# any live instance, drop the patch but no live restore.
			push_warning("[Registry] revert('scene_nodes', '%s'): field '%s' wasn't patched (or never observed on a live instance)" % [id, fname])
			continue
		if stash_per_node.has(fname):
			for root in live_roots:
				var target: Node = _resolve_scene_target(root, node_path)
				if target != null and _node_has_property(target, fname):
					target.set(fname, stash_per_node[fname])
			stash_per_node.erase(fname)
		props.erase(fname)
		pat_entry.erase(fname)
	# Prune empty nested dicts.
	if props.is_empty():
		per_node.erase(node_path)
	else:
		per_node[node_path] = props
	if per_node.is_empty():
		_scene_node_patches.erase(scene_path)
	else:
		_scene_node_patches[scene_path] = per_node
	if stash_per_node.is_empty():
		stash_per_scene.erase(node_path)
	else:
		stash_per_scene[node_path] = stash_per_node
	if stash_per_scene.is_empty():
		_scene_node_stash.erase(scene_path)
	else:
		_scene_node_stash[scene_path] = stash_per_scene
	if pat_entry.is_empty():
		patched.erase(id)
	else:
		patched[id] = pat_entry
	_registry_patched["scene_nodes"] = patched
	return true


func _collect_scene_roots(node: Node, scene_path: String, out: Array[Node]) -> void:
	if node.scene_file_path == scene_path:
		out.append(node)
		return
	for child in node.get_children():
		_collect_scene_roots(child, scene_path, out)


# ============================================================================
# Aggregators - fan-out helpers
# ============================================================================
#
# These wrap several primitive registries into a single declarative dict.
# All return Dictionary with granular per-step success bools. The
