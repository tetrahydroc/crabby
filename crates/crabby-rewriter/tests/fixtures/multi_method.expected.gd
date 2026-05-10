extends Node
class_name Example

var counter := 0

func _rtv_vanilla__ready():
	counter = 1

func _rtv_vanilla_tick(delta) -> int:
	counter += 1
	return counter

static func util() -> int:
	return 42

# --- Crabby hook dispatch wrappers ---
func _ready():
	var _lib = Engine.get_meta("RTVModLib") if Engine.has_meta("RTVModLib") else null
	if !_lib:
		_rtv_vanilla__ready()
		return
	if not _lib._any_mod_hooked:
		_rtv_vanilla__ready()
		return
	if _lib._wrapper_active.has("example-_ready"):
		_rtv_vanilla__ready()
		return
	_lib._wrapper_active["example-_ready"] = true
	var _rtv_prev_caller = _lib._caller
	_lib._caller = self
	_lib._dispatch("example-_ready-pre", [])
	var _repl = _lib._get_hooks("example-_ready")
	if _repl.size() > 0:
		var _prev_skip = _lib._skip_super
		_lib._skip_super = false
		_repl[0].callv([])
		var _did_skip = _lib._skip_super
		_lib._skip_super = _prev_skip
		if !_did_skip:
			_rtv_vanilla__ready()
	else:
		_rtv_vanilla__ready()
	_lib._caller = self
	_lib._dispatch("example-_ready-post", [])
	_lib._dispatch_deferred("example-_ready-callback", [])
	_lib._wrapper_active.erase("example-_ready")
	_lib._caller = _rtv_prev_caller

func tick(delta) -> int:
	var _lib = Engine.get_meta("RTVModLib") if Engine.has_meta("RTVModLib") else null
	if !_lib:
		return _rtv_vanilla_tick(delta)
	if not _lib._any_mod_hooked:
		return _rtv_vanilla_tick(delta)
	if _lib._wrapper_active.has("example-tick"):
		return _rtv_vanilla_tick(delta)
	_lib._wrapper_active["example-tick"] = true
	var _rtv_prev_caller = _lib._caller
	_lib._caller = self
	_lib._dispatch("example-tick-pre", [delta])
	var _result
	var _repl = _lib._get_hooks("example-tick")
	if _repl.size() > 0:
		var _prev_skip = _lib._skip_super
		_lib._skip_super = false
		var _replret = _repl[0].callv([delta])
		var _did_skip = _lib._skip_super
		_lib._skip_super = _prev_skip
		if _did_skip:
			_result = _replret
		else:
			_result = _rtv_vanilla_tick(delta)
	else:
		_result = _rtv_vanilla_tick(delta)
	_lib._caller = self
	_result = _lib._dispatch_post("example-tick-post", [delta], _result)
	_lib._dispatch_deferred("example-tick-callback", [delta])
	_lib._wrapper_active.erase("example-tick")
	_lib._caller = _rtv_prev_caller
	return _result

