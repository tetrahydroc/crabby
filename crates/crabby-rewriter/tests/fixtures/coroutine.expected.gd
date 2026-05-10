extends Control

var fade = false

func _rtv_vanilla__ready() -> void:
	fade = true
	await get_tree().create_timer(5.0).timeout
	fade = false

func _rtv_vanilla_update_fade(delta: float):
	fade = delta > 0.0

# --- Crabby hook dispatch wrappers ---
func _ready():
	var _lib = Engine.get_meta("RTVModLib") if Engine.has_meta("RTVModLib") else null
	if !_lib:
		await _rtv_vanilla__ready()
		return
	if not _lib._any_mod_hooked:
		await _rtv_vanilla__ready()
		return
	if _lib._wrapper_active.has("message-_ready"):
		await _rtv_vanilla__ready()
		return
	_lib._wrapper_active["message-_ready"] = true
	var _rtv_prev_caller = _lib._caller
	_lib._caller = self
	_lib._dispatch("message-_ready-pre", [])
	var _repl = _lib._get_hooks("message-_ready")
	if _repl.size() > 0:
		var _prev_skip = _lib._skip_super
		_lib._skip_super = false
		_repl[0].callv([])
		var _did_skip = _lib._skip_super
		_lib._skip_super = _prev_skip
		if !_did_skip:
			await _rtv_vanilla__ready()
	else:
		await _rtv_vanilla__ready()
	_lib._caller = self
	_lib._dispatch("message-_ready-post", [])
	_lib._dispatch_deferred("message-_ready-callback", [])
	_lib._wrapper_active.erase("message-_ready")
	_lib._caller = _rtv_prev_caller

func update_fade(delta: float):
	var _lib = Engine.get_meta("RTVModLib") if Engine.has_meta("RTVModLib") else null
	if !_lib:
		_rtv_vanilla_update_fade(delta)
		return
	if not _lib._any_mod_hooked:
		_rtv_vanilla_update_fade(delta)
		return
	if _lib._wrapper_active.has("message-update_fade"):
		_rtv_vanilla_update_fade(delta)
		return
	_lib._wrapper_active["message-update_fade"] = true
	var _rtv_prev_caller = _lib._caller
	_lib._caller = self
	_lib._dispatch("message-update_fade-pre", [delta])
	var _repl = _lib._get_hooks("message-update_fade")
	if _repl.size() > 0:
		var _prev_skip = _lib._skip_super
		_lib._skip_super = false
		_repl[0].callv([delta])
		var _did_skip = _lib._skip_super
		_lib._skip_super = _prev_skip
		if !_did_skip:
			_rtv_vanilla_update_fade(delta)
	else:
		_rtv_vanilla_update_fade(delta)
	_lib._caller = self
	_lib._dispatch("message-update_fade-post", [delta])
	_lib._dispatch_deferred("message-update_fade-callback", [delta])
	_lib._wrapper_active.erase("message-update_fade")
	_lib._caller = _rtv_prev_caller

