## ----- shim/lib/lib.gd -----
##
## Lib, the modding API baked directly into vanilla.
##
## Lives at res://Lib.gd inside RTV.pck (emitted as a PCK extra during
## bake) and registered as an autoload via `override.cfg` so it mounts
## before any vanilla autoload. To the engine and to mod scripts it is
## indistinguishable from a vanilla autoload like Database or LT_Master.
##
## # Boot ordering
##
## Lib is the SOLE entry in `[autoload_prepend]`, so **Lib._ready is the
## first script to run**, before any vanilla autoload's `_ready`. Boot
## orchestration is appended onto Lib via the `boot.gd` LIB_FRAGMENTS
## entry. The boot fragment's `_ready` claims
## `Engine.set_meta("RTVModLib", self)` for back-compat, mounts mod
## packs, and `call_deferred("_emit_frameworks_ready")` so mods can
## `connect()` in their own `_ready` without missing the emission.
##
## # Why a meta key?
##
## Mods written for vostok-mod-loader access the API via
## `Engine.get_meta("RTVModLib")`. Lib's own `_ready` (in boot.gd) sets
## it to `self`, so legacy mods keep working unmodified and new mods can
## use the global `Lib` autoload directly.
extends Node


const CRABBY_VERSION := "0.1.0"

signal frameworks_ready

## Hook registry. Key: `"<script>-<method>[-pre|-post|-callback]"`.
## Value: Array of { callback: Callable, priority: int, id: int }.
## Snapshot-before-iterate is enforced by `_dispatch` so hooks registered
## mid-dispatch don't fire in the current cycle (matches vostok semantics).
var _hooks: Dictionary = {}
var _next_id: int = 1

## Sticky global short-circuit. Wrappers check this first and bypass the
## whole dispatch pipeline when no mod has hooked anything this session.
var _any_mod_hooked: bool = false

## Re-entry guard per hook base. Set while a wrapper is mid-dispatch so
## nested wrappers (via super) don't double-fire.
var _wrapper_active: Dictionary = {}

## Public: set by wrappers before dispatch so callbacks can do
## `lib._caller.some_property = value` against the in-flight node.
var _caller: Node = null

## Public: flag a replace hook flips via `lib.skip_super()` to skip the
## vanilla body for this one call.
var _skip_super: bool = false

## Public: monotonic dispatch counter, useful for test ordering checks.
var _seq: int = 0

## Public: set to true once `frameworks_ready` has emitted. Mods check this
## before connecting to the signal to avoid missing it.
var _is_ready: bool = false

## One-shot deprecation warning bookkeeping for legacy 2-arg post-hook
## callbacks (those declared without the trailing `_result` parameter).
## Keyed by `"<hook_name>::<callback_object_id>"` so each (hook, mod-callback)
## pair logs at most once per session, no spam from a hot dispatch path.
var _post_legacy_warned: Dictionary = {}


## Diagnostic gate. When true, Lib emits `[Lib DIAG]` probe lines around
## hook dispatch + per-frame state samples. Leave false in shipped
## builds; flip to true and rebuild only when needed.
const _DIAG := false


# --- Diagnostic _process sampler --------------------------------------------

var _diag_sample_accum: float = 0.0
var _diag_sample_last_state: String = ""

func _process(delta: float) -> void:
	if not _DIAG:
		return
	_diag_sample_accum += delta
	if _diag_sample_accum < 2.0:
		return
	_diag_sample_accum = 0.0
	# Re-load the .tres via the loader cache. Same call FW uses; same
	# instance unless something cache-busted it. Logs `MISSING` if the
	# loader can't find it, `CACHE_REPLACED` if the instance changed
	# since last sample, or the current values otherwise.
	var path := "res://RoadToVostokEnemyAI/EnemyAISettings.tres"
	if not ResourceLoader.exists(path):
		return
	var r: Resource = load(path)
	if r == null:
		return
	# Track instance identity via get_instance_id; if it changes
	# between samples, something replaced the cached resource.
	var iid := r.get_instance_id()
	var dbg = r.get("show_debug_overlay")
	var dh = r.get("disable_hiding")
	var pi = r.get("player_invulnerable")
	var bhm = r.get("boss_health_multiplier")
	var summary := "iid=%d show_debug=%s disable_hiding=%s player_invuln=%s boss_hp_mult=%s" % [
		iid, str(dbg), str(dh), str(pi), str(bhm),
	]
	# Only print if the snapshot changed, keeps the log readable while
	# still capturing every flip.
	if summary != _diag_sample_last_state:
		print("[Lib DIAG SAMPLE] %s" % summary)
		_diag_sample_last_state = summary


## Connect scene_nodes listeners now that the tree is up, then emit
## `frameworks_ready`. Safe to call multiple times; `_is_ready` is the
## only re-entry guard.
func _emit_frameworks_ready() -> void:
	_is_ready = true
	# Lazy-connect the scene_nodes node_added listener now that the tree
	# is up. Idempotent; patches that arrive before frameworks_ready
	# also call this.
	_scene_nodes_connect_listener()
	frameworks_ready.emit()
	print("[Lib] frameworks_ready emitted")


# --- Version accessors -------------------------------------------------------

static func version() -> String:
	return CRABBY_VERSION


static func major_version() -> int:
	return int(CRABBY_VERSION.split(".")[0])


static func minor_version() -> int:
	return int(CRABBY_VERSION.split(".")[1])


static func patch_version() -> int:
	return int(CRABBY_VERSION.split(".")[2])


# --- Public hook API ---------------------------------------------------------

## Register a hook. Returns an ID usable for unhook(), or -1 if a replace
## hook is already owned for this name.
##
## Hook name format: `"<script>-<method>[-pre|-post|-callback]"`, lowercase.
## Bare name (no suffix) is a replace hook; first registration wins.
func hook(hook_name: String, callback: Callable, priority: int = 100) -> int:
	var is_replace := not (hook_name.ends_with("-pre") \
			or hook_name.ends_with("-post") \
			or hook_name.ends_with("-callback"))
	if is_replace and _hooks.has(hook_name) and (_hooks[hook_name] as Array).size() > 0:
		# Replace slots are single-owner. Caller checks the -1 return.
		if _DIAG:
			print("[Lib DIAG HOOK] REJECTED replace '%s', already owned" % hook_name)
		return -1
	var id := _next_id
	_next_id += 1
	var entry := { "callback": callback, "priority": priority, "id": id }
	if not _hooks.has(hook_name):
		_hooks[hook_name] = []
	(_hooks[hook_name] as Array).append(entry)
	(_hooks[hook_name] as Array).sort_custom(func(a, b): return a["priority"] < b["priority"])
	_any_mod_hooked = true
	if _DIAG:
		print("[Lib DIAG HOOK] REGISTERED '%s' id=%d replace=%s total=%d" % [
			hook_name, id, str(is_replace), (_hooks[hook_name] as Array).size(),
		])
	return id


## Bulk hook registration. Mirrors `hook(name, cb)` per entry; returns
## a Dictionary mapping hook names to ids (or -1 on replace conflict).
func hook_many(entries: Dictionary, priority: int = 100) -> Dictionary:
	var out := {}
	for hook_name in entries:
		var cb: Callable = entries[hook_name]
		out[hook_name] = hook(hook_name, cb, priority)
	return out


## Remove a hook by ID. Idempotent; unknown IDs no-op silently.
func unhook(hook_id: int) -> void:
	for hook_name in _hooks:
		var arr: Array = _hooks[hook_name]
		for i in range(arr.size() - 1, -1, -1):
			if arr[i]["id"] == hook_id:
				arr.remove_at(i)
				return


func has_hooks(hook_name: String) -> bool:
	return _hooks.has(hook_name) and (_hooks[hook_name] as Array).size() > 0


func has_replace(hook_name: String) -> bool:
	return _hooks.has(hook_name) and (_hooks[hook_name] as Array).size() > 0


func get_replace_owner(hook_name: String) -> int:
	if not _hooks.has(hook_name) or (_hooks[hook_name] as Array).size() == 0:
		return -1
	return (_hooks[hook_name] as Array)[0]["id"]


## Replace hooks call this to short-circuit the vanilla body for this one
## invocation. Cleared by the wrapper after the vanilla-body call site.
func skip_super() -> void:
	_skip_super = true


## Monotonic dispatch counter, for tests + debug logging.
func seq() -> int:
	return _seq


# --- Public save API ---------------------------------------------------------
#
# Mods writing per-slot state MUST use `save_path` (or one of the helpers
# layered on top later). Vanilla writes already go through Loader's
# rewrite; this exposes the same resolver to mod code so saves stay
# slot-aware (snapshots/restores cover them automatically).
#
# Direct `"user://..."` paths from mods bypass the slot system today;
# they'll land at the user-data root instead of the active slot dir,
# and won't snapshot.

## Active save slot (bare name, e.g. `"default"`).
func active_slot() -> String:
	if Engine.has_singleton("Loader"):
		return Engine.get_singleton("Loader").active_slot()
	return _read_active_slot_file().get("slot", "default")


## Active save profile (bare name, e.g. `"default"`).
func active_profile() -> String:
	if Engine.has_singleton("Loader"):
		return Engine.get_singleton("Loader").active_profile()
	return _read_active_slot_file().get("profile", "default")


## Absolute slot directory (with trailing slash). Use `save_path(name)`
## instead unless you really need the directory itself.
func save_dir() -> String:
	if Engine.has_singleton("Loader"):
		return Engine.get_singleton("Loader").save_dir()
	var t := _read_active_slot_file()
	return "user://saves/" + t.get("profile", "default") + "/" + t.get("slot", "default") + "/"


## Resolve a per-slot file path. Returns `user://saves/<profile>/<slot>/<name>`.
##
## ```gdscript
## # Mod code:
## var path := crabby.save_path("my_mod_state.cfg")
## var f := FileAccess.open(path, FileAccess.WRITE)
## ```
##
## Snapshots and slot-swaps cover everything written under this path.
func save_path(name: String) -> String:
	if Engine.has_singleton("Loader"):
		return Engine.get_singleton("Loader").save_path(name)
	return save_dir() + name


## Boot-time fallback: read `active_slot.txt` directly when the Loader
## autoload isn't available yet. Returns a Dictionary with `profile`
## and `slot` keys, same defaulting as Loader's `_rtv_init_save_slot`.
func _read_active_slot_file() -> Dictionary:
	var out := { "profile": "default", "slot": "default" }
	if not FileAccess.file_exists("user://active_slot.txt"):
		return out
	var f := FileAccess.open("user://active_slot.txt", FileAccess.READ)
	if f == null:
		return out
	var raw := f.get_as_text()
	f.close()
	for line in raw.split("\n"):
		line = line.strip_edges()
		if line.is_empty() or line.begins_with("#"):
			continue
		var eq_at := line.find("=")
		if eq_at < 0:
			continue
		var key := line.substr(0, eq_at).strip_edges()
		var value := line.substr(eq_at + 1).strip_edges()
		if not _is_safe_slot_name(value):
			continue
		if key == "profile" or key == "slot":
			out[key] = value
	return out


## Mirror of Loader's `_rtv_is_safe_slot_name` for the boot-time fallback.
func _is_safe_slot_name(name: String) -> bool:
	if name.is_empty() or name.length() > 64:
		return false
	if name.contains("/") or name.contains("\\") or name.contains(".."):
		return false
	for c in name:
		var ok := (c >= "a" and c <= "z") or (c >= "A" and c <= "Z") or (c >= "0" and c <= "9") or c == "-" or c == "_" or c == " "
		if not ok:
			return false
	return true


# --- Internal: called by rewritten wrappers ----------------------------------

## Dispatch all hooks registered under `hook_name`, in priority order.
## Arguments are passed via `callv` to match any callback signature.
##
## Snapshot-before-iterate: callbacks registering/removing hooks mid-dispatch
## see consistent semantics; their changes join the NEXT dispatch, not this
## one. Matches vostok's C03/C16/C17/C18 contracts.
func _dispatch(hook_name: String, args: Array) -> void:
	if not _hooks.has(hook_name):
		return
	var entries: Array = (_hooks[hook_name] as Array).duplicate()
	for entry in entries:
		_seq += 1
		var cb: Callable = entry["callback"]
		cb.callv(args)


## Deferred variant used for `-callback` dispatch. Each callback is
## `call_deferred`'d so it fires after the current frame settles.
func _dispatch_deferred(hook_name: String, args: Array) -> void:
	if not _hooks.has(hook_name):
		return
	var entries: Array = (_hooks[hook_name] as Array).duplicate()
	for entry in entries:
		_seq += 1
		var cb: Callable = entry["callback"]
		cb.bindv(args).call_deferred()


## Chained post-hook dispatch for non-void wrapped methods.
##
## Each callback may transform the running result by returning a non-null
## value. The contract:
##
## - **Preferred callback shape**: `func(<vanilla args...>, _result)`.
##   The trailing `_result` carries whatever the prior post hook (or the
##   vanilla body) left behind. Returning **non-null** replaces `_result`
##   for the next callback in the chain. Returning **null** is a
##   pass-through ("observed but don't want to change anything").
##
## - **Legacy callback shape**: `func(<vanilla args...>)`, no trailing
##   `_result`. The dispatcher detects arity via
##   `Callable.get_argument_count()` and falls back to fire-and-forget
##   on args only, ignoring the return. A one-shot deprecation warning
##   per (hook_name, callback) pair guides authors to update.
##
## Snapshot-before-iterate: same rationale as `_dispatch`; mid-dispatch
## hook()/unhook() calls don't disturb the in-flight pass.
##
## Limitation: a hook that genuinely wants to return literal `null` to
## replace `_result` can't model that here; null is the pass-through
## sentinel. Use a pre-hook + `skip_super()` instead for literal-null
## replacement.
func _dispatch_post(hook_name: String, args: Array, current_result: Variant) -> Variant:
	if not _hooks.has(hook_name):
		return current_result
	var entries: Array = (_hooks[hook_name] as Array).duplicate()
	var expected_with_result: int = args.size() + 1
	for entry in entries:
		_seq += 1
		var cb: Callable = entry["callback"]
		var argc: int = cb.get_argument_count()
		var ret: Variant = null
		if argc == expected_with_result:
			ret = cb.callv(args + [current_result])
		else:
			var warn_key := "%s::%d" % [hook_name, cb.get_object_id()]
			if not _post_legacy_warned.has(warn_key):
				_post_legacy_warned[warn_key] = true
				push_warning(
					"[Lib] post hook '%s' callback uses legacy %d-arg signature (expected %d for non-void wrapper). Add a trailing _result param to your callback to receive + optionally mutate the return value; the legacy form will be removed in a future major version." \
					% [hook_name, argc, expected_with_result]
				)
			cb.callv(args)
		if ret != null:
			current_result = ret
	return current_result


## Return the list of registered callbacks under a hook name, in priority
## order. Used by rewritten wrappers to resolve replace hooks.
func _get_hooks(hook_name: String) -> Array:
	if _DIAG and hook_name.begins_with("aispawner-"):
		_diag_log_aispawner_state(hook_name, "PROBE")
	if not _hooks.has(hook_name):
		return []
	var callbacks := []
	for entry in _hooks[hook_name]:
		# When emitted for an aispawner-* hook, wrap the callback in
		# a diagnostic shim that logs the spawner state before AND
		# after the callback runs. Catches "callback ran but its
		# mutations didn't stick" vs "callback never ran." The
		# wrapper Callable is one-shot per probe.
		if _DIAG and hook_name.begins_with("aispawner-"):
			callbacks.append(_diag_wrap_callback(hook_name, entry["callback"]))
		else:
			callbacks.append(entry["callback"])
	return callbacks


# Wrap a real callback to log spawner state immediately after it
# returns. Bound via Callable.bind to capture the hook name + original
# cb in the wrapper.
func _diag_wrap_callback(hook_name: String, cb: Callable) -> Callable:
	return func(): _diag_run_and_log(hook_name, cb, [])

func _diag_run_and_log(hook_name: String, cb: Callable, args: Array) -> void:
	_diag_log_aispawner_state(hook_name, "BEFORE_CB")
	cb.callv(args)
	_diag_log_aispawner_state(hook_name, "AFTER_CB")

func _diag_log_aispawner_state(hook_name: String, label: String) -> void:
	var c := _caller
	if c == null:
		print("[Lib DIAG %s] '%s' <no caller>" % [label, hook_name])
		return
	var sp = c.get("spawnPool")
	var sl = c.get("spawnLimit")
	var nh = c.get("noHiding")
	var ag = c.get("agent")
	var probe_write := ""
	if label == "AFTER_CB" and hook_name == "aispawner-_ready":
		var sh_node: Node = get_tree().root.get_node_or_null("EnemyAIMain/SpawnerHooks")
		if sh_node != null:
			var sh_settings = sh_node.get("EnemyAISettings")
			if sh_settings != null:
				var ip = sh_settings.get("intensity_preset")
				var spb = sh_settings.get("spawn_pool_bonus")
				probe_write += " | sh ip=%s(%s) spb=%s(%s)" % [
					str(ip), type_string(typeof(ip)),
					str(spb), type_string(typeof(spb)),
				]
			if sh_node.has_method("_preset_profile"):
				var pp = sh_node.call("_preset_profile")
				if pp is Dictionary:
					probe_write += " | _preset_profile.spawn_pool=%s(%s)" % [
						str(pp.get("spawn_pool")),
						type_string(typeof(pp.get("spawn_pool"))),
					]
				else:
					probe_write += " | _preset_profile NOT a Dict: %s" % str(pp)
			if sh_node.has_method("_spawn_pool"):
				var result = sh_node.call("_spawn_pool")
				probe_write += " | _spawn_pool()=%s(%s)" % [
					str(result), type_string(typeof(result)),
				]
		else:
			probe_write += " | SpawnerHooks node not found at expected path"
	print("[Lib DIAG %s] '%s' caller=%s spawnPool=%s spawnLimit=%s noHiding=%s agent=%s%s" % [
		label, hook_name, str(c.get_instance_id()),
		str(sp), str(sl), str(nh),
		"<set>" if ag != null else "<null>",
		probe_write,
	])
