extends GPUParticles3D
class_name Flash

var duration := 0.05

func _rtv_vanilla__ready():
	emitting = true

func _rtv_vanilla__process(delta):
	duration -= delta
	if duration <= 0.0:
		queue_free()

# --- Crabby hook dispatch wrappers ---
func _ready():
	var _lib = Engine.get_meta("RTVModLib") if Engine.has_meta("RTVModLib") else null
	if not _lib or not _lib._any_mod_hooked:
		_rtv_vanilla__ready()
		return
	_lib._caller = self
	_lib._dispatch("muzzleflash-_ready-pre", [])
	var _repl = _lib._get_hooks("muzzleflash-_ready")
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
	_lib._dispatch("muzzleflash-_ready-post", [])
	_lib._dispatch_deferred("muzzleflash-_ready-callback", [])

func _process(delta):
	var _lib = Engine.get_meta("RTVModLib") if Engine.has_meta("RTVModLib") else null
	if not _lib or not _lib._any_mod_hooked:
		_rtv_vanilla__process(delta)
		return
	_lib._caller = self
	_lib._dispatch("muzzleflash-_process-pre", [delta])
	var _repl = _lib._get_hooks("muzzleflash-_process")
	if _repl.size() > 0:
		var _prev_skip = _lib._skip_super
		_lib._skip_super = false
		_repl[0].callv([delta])
		var _did_skip = _lib._skip_super
		_lib._skip_super = _prev_skip
		if !_did_skip:
			_rtv_vanilla__process(delta)
	else:
		_rtv_vanilla__process(delta)
	_lib._dispatch("muzzleflash-_process-post", [delta])
	_lib._dispatch_deferred("muzzleflash-_process-callback", [delta])

