# ---- sounds kind ----
#
# AudioLibrary is a plain Resource at res://Resources/AudioLibrary.tres
# (not an autoload). Vanilla scripts hardcode property names like
# `audioLibrary.knifeSlash`. Mutating those fields propagates to every
# holder (Godot's resource cache). Mod-registered ids that don't exist
# on the library can only be reached via lib.get_entry; vanilla scripts
# can't see them.

const _AUDIO_LIBRARY_PATH := "res://Resources/AudioLibrary.tres"

var _audio_library_cache: Resource = null
var _audio_library_warned: bool = false


func _audio_library() -> Resource:
	if _audio_library_cache != null:
		return _audio_library_cache
	var lib: Resource = load(_AUDIO_LIBRARY_PATH)
	if lib == null:
		if not _audio_library_warned:
			push_warning("[Registry] sounds: AudioLibrary.tres missing at %s; sounds registry is inert" % _AUDIO_LIBRARY_PATH)
			_audio_library_warned = true
		return null
	_audio_library_cache = lib
	return lib


## Coerce input data into an AudioEvent Resource. Accepts:
##   - AudioEvent-shaped Resource (pass-through)
##   - bare AudioStream (wrap in default-constructed AudioEvent)
##   - Dictionary {audioClips, volume, randomPitch} (sensible defaults)
func _coerce_audio_event(id: String, verb: String, data: Variant) -> Resource:
	if data is Resource and _looks_like_audio_event(data):
		return data
	if data is AudioStream:
		var ev_class: GDScript = _audio_event_class()
		if ev_class == null:
			push_warning("[Registry] %s('sounds', '%s'): couldn't locate AudioEvent class (library may be empty or unmigrated)" % [verb, id])
			return null
		var ev: Resource = ev_class.new()
		ev.set("audioClips", [data])
		ev.set("volume", 0.0)
		ev.set("randomPitch", false)
		return ev
	if data is Dictionary:
		var d: Dictionary = data
		var ev_class_d: GDScript = _audio_event_class()
		if ev_class_d == null:
			push_warning("[Registry] %s('sounds', '%s'): couldn't locate AudioEvent class to construct from dict" % [verb, id])
			return null
		var ev: Resource = ev_class_d.new()
		if d.has("audioClips"):
			ev.set("audioClips", d["audioClips"])
		else:
			ev.set("audioClips", [])
		ev.set("volume", float(d.get("volume", 0.0)))
		ev.set("randomPitch", bool(d.get("randomPitch", false)))
		return ev
	push_warning("[Registry] %s('sounds', '%s', ...) expects AudioEvent / AudioStream / Dictionary, got %s" % [verb, id, typeof(data)])
	return null


## Walk the live AudioLibrary for the first non-null @export AudioEvent;
## use its script as the AudioEvent class reference. Avoids hardcoding
## the AudioEvent.gd path (which the game could move).
func _audio_event_class() -> GDScript:
	var lib: Resource = _audio_library()
	if lib == null:
		return null
	for p in lib.get_property_list():
		var pname: Variant = p.get("name")
		if not (int(p.get("usage", 0)) & PROPERTY_USAGE_SCRIPT_VARIABLE):
			continue
		var val: Variant = lib.get(pname)
		if val == null or not (val is Resource):
			continue
		var s: GDScript = val.get_script()
		if s != null:
			return s
	return null


func _looks_like_audio_event(res: Resource) -> bool:
	return _resource_has_property(res, "audioClips") \
			and _resource_has_property(res, "volume") \
			and _resource_has_property(res, "randomPitch")


func _sound_exists_in_vanilla(id: String) -> bool:
	var lib: Resource = _audio_library()
	if lib == null:
		return false
	return _resource_has_property(lib, id)


## Lookup precedence: mod overrides > mod registrations > vanilla library
## field. Overrides on vanilla names live as `set()` mutations on the
## library itself; lookups via `audioLibrary.get(id)` would find them
## there. Routed through `_registry_registered` first to keep the
## registry self-contained.
func _lookup_sound(id: String) -> Resource:
	var reg: Dictionary = _registry_registered.get("sounds", {})
	if reg.has(id):
		return reg[id]
	var lib: Resource = _audio_library()
	if lib == null:
		return null
	if _resource_has_property(lib, id):
		return lib.get(id)
	return null


func _register_sound(id: String, data: Variant) -> bool:
	if _sound_exists_in_vanilla(id):
		push_warning("[Registry] register('sounds', '%s'): id collides with vanilla AudioLibrary field; use override instead" % id)
		return false
	var reg: Dictionary = _registry_registered.get("sounds", {})
	if reg.has(id):
		push_warning("[Registry] register('sounds', '%s'): already registered by a mod" % id)
		return false
	var ev: Resource = _coerce_audio_event(id, "register", data)
	if ev == null:
		return false
	reg[id] = ev
	_registry_registered["sounds"] = reg
	return true


func _override_sound(id: String, data: Variant) -> bool:
	var lib: Resource = _audio_library()
	if lib == null:
		return false
	if not _sound_exists_in_vanilla(id):
		push_warning("[Registry] override('sounds', '%s'): no vanilla AudioLibrary field with that name (register can't be overridden; revert the register first)" % id)
		return false
	var ev: Resource = _coerce_audio_event(id, "override", data)
	if ev == null:
		return false
	# First-write-wins stash so multiple overrides on the same id still
	# restore to true vanilla on revert.
	var ov: Dictionary = _registry_overridden.get("sounds", {})
	if not ov.has(id):
		ov[id] = lib.get(id)
		_registry_overridden["sounds"] = ov
	lib.set(id, ev)
	return true


func _patch_sound(id: String, fields: Dictionary) -> bool:
	if fields.is_empty():
		push_warning("[Registry] patch('sounds', '%s', ...): empty fields dict is a no-op" % id)
		return false
	var target: Resource = _lookup_sound(id)
	if target == null:
		push_warning("[Registry] patch('sounds', '%s'): no sound with that id" % id)
		return false
	var patched: Dictionary = _registry_patched.get("sounds", {})
	var stash: Dictionary = patched.get(id, {})
	for field in fields.keys():
		var field_name := String(field)
		if not _resource_has_property(target, field_name):
			push_warning("[Registry] patch('sounds', '%s'): field '%s' doesn't exist on AudioEvent (valid: audioClips, volume, randomPitch)" \
					% [id, field_name])
			continue
		if not stash.has(field_name):
			stash[field_name] = target.get(field_name)
		target.set(field_name, fields[field])
	patched[id] = stash
	_registry_patched["sounds"] = patched
	return true


func _remove_sound(id: String) -> bool:
	var reg: Dictionary = _registry_registered.get("sounds", {})
	if not reg.has(id):
		push_warning("[Registry] remove('sounds', '%s'): not registered by a mod" % id)
		return false
	# Sounds don't have items-style override-lives-in-registered dual
	# storage; overrides mutate the library directly, not this dict.
	reg.erase(id)
	_registry_registered["sounds"] = reg
	return true


func _revert_sound(id: String, fields: Array) -> bool:
	var did_something := false
	var ov: Dictionary = _registry_overridden.get("sounds", {})
	var patched: Dictionary = _registry_patched.get("sounds", {})
	var lib: Resource = _audio_library()
	if fields.is_empty():
		if patched.has(id):
			var target: Resource = _lookup_sound(id)
			if target != null:
				var stash: Dictionary = patched[id]
				for fname in stash.keys():
					target.set(fname, stash[fname])
			patched.erase(id)
			_registry_patched["sounds"] = patched
			did_something = true
		if ov.has(id) and lib != null:
			lib.set(id, ov[id])
			ov.erase(id)
			_registry_overridden["sounds"] = ov
			did_something = true
		if not did_something:
			push_warning("[Registry] revert('sounds', '%s'): nothing to revert" % id)
		return did_something
	if not patched.has(id):
		push_warning("[Registry] revert('sounds', '%s', %s): no patches on this id" % [id, fields])
		return false
	var target: Resource = _lookup_sound(id)
	if target == null:
		push_warning("[Registry] revert('sounds', '%s', %s): id no longer resolves" % [id, fields])
		return false
	var stash: Dictionary = patched[id]
	for field in fields:
		var fname := String(field)
		if not stash.has(fname):
			push_warning("[Registry] revert('sounds', '%s'): field '%s' wasn't patched" % [id, fname])
			continue
		target.set(fname, stash[fname])
		stash.erase(fname)
		did_something = true
	if stash.is_empty():
		patched.erase(id)
	else:
		patched[id] = stash
	_registry_patched["sounds"] = patched
	return did_something


# ---- array-ops on sound Resource fields ----

func _append_sound(id: String, field: String, values: Array, allow_duplicates: bool) -> bool:
	var target: Resource = _lookup_sound(id)
	if target == null:
		push_warning("[Registry] append('sounds', '%s'): no sound with that id" % id)
		return false
	return _array_op_on_resource("sounds", id, target, field, "append", values, allow_duplicates)


func _prepend_sound(id: String, field: String, values: Array, allow_duplicates: bool) -> bool:
	var target: Resource = _lookup_sound(id)
	if target == null:
		push_warning("[Registry] prepend('sounds', '%s'): no sound with that id" % id)
		return false
	return _array_op_on_resource("sounds", id, target, field, "prepend", values, allow_duplicates)


func _remove_from_sound(id: String, field: String, values: Array) -> bool:
	var target: Resource = _lookup_sound(id)
	if target == null:
		push_warning("[Registry] remove_from('sounds', '%s'): no sound with that id" % id)
		return false
	return _array_op_on_resource("sounds", id, target, field, "remove_from", values, false)
