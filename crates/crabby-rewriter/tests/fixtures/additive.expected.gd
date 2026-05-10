extends Resource
class_name WorldSave

@export var data: Dictionary = {}

func save_data(dict: Dictionary) -> void:
	data = dict

func has_key(key: String) -> bool:
	return data.has(key)

# --- Crabby hook dispatch wrappers ---
func _rtv_hooked_save_data(dict: Dictionary) -> void:
	var _lib = Engine.get_meta("RTVModLib") if Engine.has_meta("RTVModLib") else null
	if !_lib:
		self.save_data(dict)
		return
	if not _lib._any_mod_hooked:
		self.save_data(dict)
		return
	if _lib._wrapper_active.has("worldsave-save_data"):
		self.save_data(dict)
		return
	_lib._wrapper_active["worldsave-save_data"] = true
	var _rtv_prev_caller = _lib._caller
	_lib._caller = self
	_lib._dispatch("worldsave-save_data-pre", [dict])
	var _repl = _lib._get_hooks("worldsave-save_data")
	if _repl.size() > 0:
		var _prev_skip = _lib._skip_super
		_lib._skip_super = false
		_repl[0].callv([dict])
		var _did_skip = _lib._skip_super
		_lib._skip_super = _prev_skip
		if !_did_skip:
			self.save_data(dict)
	else:
		self.save_data(dict)
	_lib._caller = self
	_lib._dispatch("worldsave-save_data-post", [dict])
	_lib._dispatch_deferred("worldsave-save_data-callback", [dict])
	_lib._wrapper_active.erase("worldsave-save_data")
	_lib._caller = _rtv_prev_caller

func _rtv_hooked_has_key(key: String) -> bool:
	var _lib = Engine.get_meta("RTVModLib") if Engine.has_meta("RTVModLib") else null
	if !_lib:
		return self.has_key(key)
	if not _lib._any_mod_hooked:
		return self.has_key(key)
	if _lib._wrapper_active.has("worldsave-has_key"):
		return self.has_key(key)
	_lib._wrapper_active["worldsave-has_key"] = true
	var _rtv_prev_caller = _lib._caller
	_lib._caller = self
	_lib._dispatch("worldsave-has_key-pre", [key])
	var _result
	var _repl = _lib._get_hooks("worldsave-has_key")
	if _repl.size() > 0:
		var _prev_skip = _lib._skip_super
		_lib._skip_super = false
		var _replret = _repl[0].callv([key])
		var _did_skip = _lib._skip_super
		_lib._skip_super = _prev_skip
		if _did_skip:
			_result = _replret
		else:
			_result = self.has_key(key)
	else:
		_result = self.has_key(key)
	_lib._caller = self
	_result = _lib._dispatch_post("worldsave-has_key-post", [key], _result)
	_lib._dispatch_deferred("worldsave-has_key-callback", [key])
	_lib._wrapper_active.erase("worldsave-has_key")
	_lib._caller = _rtv_prev_caller
	return _result

