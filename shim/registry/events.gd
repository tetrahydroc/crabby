# ---- events kind ----
#
# Events.tres holds a single `events: Array[EventData]` that EventSystem
# const-preloads at _ready() into per-type buckets. Same timing
# constraint as loot/recipes; mods must register during their own
# `_ready()` for additions to land in EventSystem before the first
# GetAvailableEvents() call.
#
# patch and revert accept either a String handle or a direct EventData
# Resource ref, so vanilla events can be patched without first
# registering a handle.
#
# Caveat for mod authors: EventData.function names a method on
# EventSystem. Registering an event whose `function` references a
# method EventSystem doesn't have makes it a no-op when it fires.

const _EVENTS_PATH := "res://Events/Events.tres"

var _events_cache: Resource = null
var _events_warned: bool = false


func _events_resource() -> Resource:
	if _events_cache != null:
		return _events_cache
	var res: Resource = load(_EVENTS_PATH)
	if res == null:
		if not _events_warned:
			push_warning("[Registry] events: Events.tres missing at %s; events registry is inert" % _EVENTS_PATH)
			_events_warned = true
		return null
	_events_cache = res
	return res


func _looks_like_event_data(res: Resource) -> bool:
	return _resource_has_property(res, "function") \
			and _resource_has_property(res, "possibility") \
			and _resource_has_property(res, "day")


func _validate_event_data(id: String, verb: String, data: Variant) -> Array:
	if not (data is Dictionary):
		push_warning("[Registry] %s('events', '%s', ...) expects Dictionary {event}, got %s" % [verb, id, typeof(data)])
		return [null, null]
	var d: Dictionary = data
	if not d.has("event"):
		push_warning("[Registry] %s('events', '%s', ...) data dict missing 'event' key" % [verb, id])
		return [null, null]
	var event: Variant = d["event"]
	if not (event is Resource) or not _looks_like_event_data(event):
		push_warning("[Registry] %s('events', '%s'): event is not an EventData Resource" % [verb, id])
		return [null, null]
	var events_res: Resource = _events_resource()
	if events_res == null:
		return [null, null]
	var arr: Variant = events_res.get("events")
	if not (arr is Array):
		push_warning("[Registry] %s('events', '%s'): Events.events is not an Array" % [verb, id])
		return [null, null]
	return [event, arr]


func _register_event(id: String, data: Variant) -> bool:
	if _mod_added_has("events", id):
		push_warning("[Registry] register('events', '%s'): already registered (pick a unique handle)" % id)
		return false
	var parts: Array = _validate_event_data(id, "register", data)
	var event: Variant = parts[0]
	var arr: Variant = parts[1]
	if event == null or arr == null:
		return false
	if not _typed_array_accepts(arr, event):
		push_warning("[Registry] register('events', '%s'): event type doesn't match Events.events typed array" % id)
		return false
	if event in arr:
		push_warning("[Registry] register('events', '%s'): event already present; use override instead" % id)
		return false
	arr.append(event)
	# Vanilla `Events.tres._id_index` (injected by events_index_transform)
	# is the canonical store. Mod-supplied id is the index key.
	var events_res: Resource = _events_resource()
	if events_res != null and events_res.has_method("_index_add"):
		events_res._index_add(id, event)
	_mod_added_mark("events", id)
	return true


func _override_event(id: String, data: Variant) -> bool:
	var ov: Dictionary = _registry_overridden.get("events", {})
	if ov.has(id):
		push_warning("[Registry] override('events', '%s'): already overridden (revert first to re-override)" % id)
		return false
	if not (data is Dictionary) or not data.has("replaces"):
		push_warning("[Registry] override('events', '%s', ...) requires {event, replaces: EventData}" % id)
		return false
	var parts: Array = _validate_event_data(id, "override", data)
	var new_event: Variant = parts[0]
	var arr: Variant = parts[1]
	if new_event == null or arr == null:
		return false
	var old_event: Variant = data["replaces"]
	if not (old_event is Resource) or not _looks_like_event_data(old_event):
		push_warning("[Registry] override('events', '%s'): 'replaces' is not an EventData Resource" % id)
		return false
	if not _typed_array_accepts(arr, new_event):
		push_warning("[Registry] override('events', '%s'): event type doesn't match Events.events typed array" % id)
		return false
	var idx: int = arr.find(old_event)
	if idx < 0:
		push_warning("[Registry] override('events', '%s'): 'replaces' not present in Events.events" % id)
		return false
	if new_event in arr:
		push_warning("[Registry] override('events', '%s'): new event already in array; would duplicate" % id)
		return false
	arr[idx] = new_event
	# Drive the vanilla-side _id_index. Override slot is the file-stem of
	# the displaced (vanilla) event, same key already in the index.
	var slot_key: String = ""
	var events_res: Resource = _events_resource()
	if events_res != null:
		if old_event.resource_path != "":
			slot_key = old_event.resource_path.get_file().get_basename()
		if slot_key != "" and events_res.has_method("_index_set"):
			events_res._index_set(slot_key, new_event)
	ov[id] = {
		"event": new_event,
		"replaced": old_event,
		"index": idx,
		"slot_key": slot_key,
	}
	_registry_overridden["events"] = ov
	_mod_added_mark("events", id)
	return true


## Resolve String handle OR direct EventData ref to (target, patch_key).
## patch_key is the stable key used in _registry_patched: handle string
## for handles, "ref:<instance_id>" for direct refs.
##
## String handles resolve via vanilla `Events.tres._id_index`; mod
## handles AND vanilla file-stems both work as patch ids.
func _resolve_event_patch_target(id: Variant) -> Array:
	if id is String:
		var events_res: Resource = _events_resource()
		if events_res != null and "_id_index" in events_res:
			var entry: Variant = events_res._id_index.get(id)
			if entry is Resource:
				return [entry, id]
		push_warning("[Registry] patch('events', '%s'): no event with that id (vanilla file-stem or mod handle)" % id)
		return [null, null]
	if id is Resource and _looks_like_event_data(id):
		return [id, "ref:%d" % id.get_instance_id()]
	push_warning("[Registry] patch('events', ...): id must be a String handle or an EventData Resource")
	return [null, null]


func _patch_event(id: Variant, fields: Dictionary) -> bool:
	if fields.is_empty():
		push_warning("[Registry] patch('events', ...): empty fields dict is a no-op")
		return false
	var resolved: Array = _resolve_event_patch_target(id)
	var target: Variant = resolved[0]
	var key: Variant = resolved[1]
	if target == null:
		return false
	var patched: Dictionary = _registry_patched.get("events", {})
	var stash: Dictionary = patched.get(key, {})
	for field in fields.keys():
		var fname := String(field)
		if not _resource_has_property(target, fname):
			push_warning("[Registry] patch('events'): field '%s' doesn't exist on EventData" % fname)
			continue
		if not stash.has(fname):
			stash[fname] = target.get(fname)
		target.set(fname, fields[field])
	patched[key] = stash
	_registry_patched["events"] = patched
	return true


func _remove_event(id: String) -> bool:
	if not _mod_added_has("events", id):
		push_warning("[Registry] remove('events', '%s'): not a mod event registration" % id)
		return false
	var ov: Dictionary = _registry_overridden.get("events", {})
	if ov.has(id):
		push_warning("[Registry] remove('events', '%s'): entry is an override, use revert instead" % id)
		return false
	var events_res: Resource = _events_resource()
	if events_res == null or not ("_id_index" in events_res):
		_mod_added_unmark("events", id)
		return false
	var event: Variant = events_res._id_index.get(id)
	if event is Resource:
		var arr: Variant = events_res.get("events")
		if arr is Array:
			var idx: int = arr.find(event)
			if idx >= 0:
				arr.remove_at(idx)
			else:
				push_warning("[Registry] remove('events', '%s'): event not found in array; tracking cleared" % id)
	if events_res.has_method("_index_remove"):
		events_res._index_remove(id)
	_mod_added_unmark("events", id)
	return true


func _revert_event(id: Variant, fields: Array) -> bool:
	var did_something := false
	var ov: Dictionary = _registry_overridden.get("events", {})
	var patched: Dictionary = _registry_patched.get("events", {})
	# Resolve patch key + target (mirrors _resolve_event_patch_target).
	var patch_key: Variant = null
	var patch_target: Resource = null
	if id is String:
		patch_key = id
		var events_for_target: Resource = _events_resource()
		if events_for_target != null and "_id_index" in events_for_target:
			var entry_t: Variant = events_for_target._id_index.get(id)
			if entry_t is Resource:
				patch_target = entry_t
	elif id is Resource and _looks_like_event_data(id):
		patch_key = "ref:%d" % id.get_instance_id()
		patch_target = id
	if fields.is_empty():
		if patch_key != null and patched.has(patch_key):
			if patch_target != null:
				var stash: Dictionary = patched[patch_key]
				for fname in stash.keys():
					patch_target.set(fname, stash[fname])
			patched.erase(patch_key)
			_registry_patched["events"] = patched
			did_something = true
		if id is String and ov.has(id):
			var entry: Dictionary = ov[id]
			var events_res: Resource = _events_resource()
			if events_res != null:
				var arr: Variant = events_res.get("events")
				if arr is Array:
					var current_idx: int = arr.find(entry["event"])
					if current_idx >= 0:
						arr[current_idx] = entry["replaced"]
					else:
						push_warning("[Registry] revert('events', '%s'): override's event missing from array, appending original at end" % id)
						arr.append(entry["replaced"])
				# Restore the vanilla-side index slot to point at the
				# displaced original.
				var slot_key: String = entry.get("slot_key", "")
				if slot_key != "" and events_res.has_method("_index_set"):
					events_res._index_set(slot_key, entry["replaced"])
			ov.erase(id)
			_registry_overridden["events"] = ov
			_mod_added_unmark("events", id)
			did_something = true
		if not did_something:
			push_warning("[Registry] revert('events'): nothing to revert for that id")
		return did_something
	if patch_key == null or not patched.has(patch_key):
		push_warning("[Registry] revert('events'): no patches found for that id")
		return false
	if patch_target == null:
		push_warning("[Registry] revert('events'): patch target no longer resolves")
		return false
	var stash2: Dictionary = patched[patch_key]
	for field in fields:
		var fname := String(field)
		if not stash2.has(fname):
			push_warning("[Registry] revert('events'): field '%s' wasn't patched" % fname)
			continue
		patch_target.set(fname, stash2[fname])
		stash2.erase(fname)
		did_something = true
	if stash2.is_empty():
		patched.erase(patch_key)
	else:
		patched[patch_key] = stash2
	_registry_patched["events"] = patched
	return did_something


# ---- array-ops on event Resource fields ----

func _append_event(id: Variant, field: String, values: Array, allow_duplicates: bool) -> bool:
	var resolved: Array = _resolve_event_patch_target(id)
	var target: Variant = resolved[0]
	var key: Variant = resolved[1]
	if target == null:
		return false
	return _array_op_on_resource("events", key, target, field, "append", values, allow_duplicates)


func _prepend_event(id: Variant, field: String, values: Array, allow_duplicates: bool) -> bool:
	var resolved: Array = _resolve_event_patch_target(id)
	var target: Variant = resolved[0]
	var key: Variant = resolved[1]
	if target == null:
		return false
	return _array_op_on_resource("events", key, target, field, "prepend", values, allow_duplicates)


func _remove_from_event(id: Variant, field: String, values: Array) -> bool:
	var resolved: Array = _resolve_event_patch_target(id)
	var target: Variant = resolved[0]
	var key: Variant = resolved[1]
	if target == null:
		return false
	return _array_op_on_resource("events", key, target, field, "remove_from", values, false)
