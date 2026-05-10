extends CharacterBody3D
class_name Controller

var walk_speed := 5.0

func _rtv_vanilla__physics_process(delta):
	velocity += Vector3(0, -9.8, 0) * delta
	move_and_slide()

func _input(event):
	pass

# --- Crabby hook dispatch wrapper ---
func _physics_process(delta):
	var _lib = Engine.get_meta("RTVModLib") if Engine.has_meta("RTVModLib") else null
	if !_lib:
		_rtv_vanilla__physics_process(delta)
		return
	if not _lib._any_mod_hooked:
		_rtv_vanilla__physics_process(delta)
		return
	if _lib._wrapper_active.has("controller-_physics_process"):
		_rtv_vanilla__physics_process(delta)
		return
	_lib._wrapper_active["controller-_physics_process"] = true
	var _rtv_prev_caller = _lib._caller
	_lib._caller = self
	_lib._dispatch("controller-_physics_process-pre", [delta])
	var _repl = _lib._get_hooks("controller-_physics_process")
	if _repl.size() > 0:
		var _prev_skip = _lib._skip_super
		_lib._skip_super = false
		_repl[0].callv([delta])
		var _did_skip = _lib._skip_super
		_lib._skip_super = _prev_skip
		if !_did_skip:
			_rtv_vanilla__physics_process(delta)
	else:
		_rtv_vanilla__physics_process(delta)
	_lib._caller = self
	_lib._dispatch("controller-_physics_process-post", [delta])
	_lib._dispatch_deferred("controller-_physics_process-callback", [delta])
	_lib._wrapper_active.erase("controller-_physics_process")
	_lib._caller = _rtv_prev_caller
