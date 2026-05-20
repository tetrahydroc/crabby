## ----- shim/lib/boot.gd -----
##
## Boot orchestration baked into Lib. Lib's `_ready` is the single entry
## point: sets `Engine.set_meta("RTVModLib", self)` for legacy mods,
## mounts every enabled mod pack, then defers `_emit_frameworks_ready()`
## so mods connecting to `frameworks_ready` in their own `_ready` don't
## miss the emission.
##
## Helper machinery (mod_index reading, archive scanning, vmz->zip cache
## fallback, folder->zip cache, mod.txt parsing, autoload instantiation,
## manifest preprocessing) lives here. Each function is appended onto
## Lib's class via the LIB_FRAGMENTS concat (see crabby-install::artifacts),
## so they call peer state on Lib (e.g. `_DIAG`) without any indirection.
##
## # Ordering invariant
##
## Lib is the SOLE [autoload_prepend] entry, so it runs before vanilla
## autoloads' `_ready` fires. When this code calls
## `ProjectSettings.load_resource_pack(...)`, the mounted res:// paths
## are visible BEFORE the game's autoloads start parsing their
## `preload(...)` lines. That's the whole reason mods can hook the
## vanilla bake.

## Boot orchestrator. Runs as Lib's `_ready` thanks to LIB_FRAGMENTS
## concat order (the sole _ready on the class).
func _ready() -> void:
	# Self-meta for legacy mods: anything that does
	# `Engine.get_meta("RTVModLib")` (the vostok-mod-loader convention)
	# gets Lib directly.
	if Engine.has_meta("RTVModLib"):
		push_warning("[Lib] RTVModLib already set - another loader is installed; crabby will not overwrite")
	else:
		Engine.set_meta("RTVModLib", self)
		print("[Lib] Engine.meta('RTVModLib') -> /root/Lib")

	# Mount user-enabled mods. MUST happen in `_ready` (not deferred or
	# later) because mod autoloads need to be in the tree before any
	# vanilla autoload's `_ready` runs. The vanilla scripts to hook are
	# already rewritten in RTV.pck (PCK-rewrite install path); no side
	# pack to mount here.
	_mount_active_mods()

	# Defer frameworks_ready so mods that connect to the signal in their
	# own `_ready` (which fires after Lib's, since their autoloads come
	# from project.binary's [autoload]) don't miss the emission.
	call_deferred("_emit_frameworks_ready")


## Read mod_config.cfg, locate each enabled mod's archive, mount it,
## and instantiate its declared autoloads under /root. Failures are
## non-fatal per-mod, one broken mod must not prevent the rest from
## loading.
func _mount_active_mods() -> void:
	var game_dir := OS.get_executable_path().get_base_dir()
	var config_path := game_dir.path_join(".crabby/mod_config.cfg")
	if not FileAccess.file_exists(config_path):
		print("[Lib] no mod_config.cfg at %s, no mods to load" % config_path)
		return

	var cfg := ConfigFile.new()
	var err := cfg.load(config_path)
	if err != OK:
		push_warning("[Lib] mod_config.cfg parse failed at %s (err=%d), skipping mod load" % [config_path, err])
		return

	var schema_version: int = cfg.get_value("crabby", "schema_version", -1)
	if schema_version != 1:
		push_warning("[Lib] mod_config.cfg schema_version=%d (expected 1), skipping" % schema_version)
		return

	var profile_name: String = cfg.get_value("crabby", "active_profile", "")
	if profile_name.is_empty():
		push_warning("[Lib] mod_config.cfg has no active_profile, skipping")
		return

	var section := "profile.%s" % profile_name
	if not cfg.has_section(section):
		push_warning("[Lib] mod_config.cfg active_profile=%s missing, skipping" % profile_name)
		return

	# Build the ordered root list:
	#   1. dev roots (in [crabby.roots] order)
	#   2. <game-dir>/Mods/
	#   3. non-dev roots (in [crabby.roots] order)
	# A mod with the same id in a higher-precedence root wins. Lets devs
	# point at a checkout and have their edits show without copying.
	var dev_roots: Array[String] = []
	var nondev_roots: Array[String] = []
	if cfg.has_section("crabby.roots"):
		for key in cfg.get_section_keys("crabby.roots"):
			var entry: Variant = cfg.get_value("crabby.roots", key)
			if typeof(entry) != TYPE_DICTIONARY:
				push_warning("[Lib] [crabby.roots] %s: malformed entry, skipping" % key)
				continue
			var path: String = entry.get("path", "")
			if path.is_empty():
				continue
			var is_dev: bool = entry.get("dev", false)
			if is_dev:
				dev_roots.append(path)
			else:
				nondev_roots.append(path)
	var roots: Array[String] = []
	roots.append_array(dev_roots)
	roots.append(game_dir.path_join("Mods"))
	roots.append_array(nondev_roots)

	# Pre-resolved id -> archive path map written by the launcher. Each
	# entry is the on-disk location of an enabled mod. Disabled mods are
	# absent by construction. Consulted first so the boot path doesn't
	# open every archive in `Mods/` looking for matches; disabled-mod
	# archives stay completely untouched.
	var mod_index: Dictionary = _load_mod_index(game_dir)

	# Build the enabled-mods list, then sort by (priority, mod_name,
	# filename) before mounting. Lower `priority` value loads first,
	# matches vostok-mod-loader's `_compare_load_order` exactly. Matters
	# for mods like MCM that declare `priority=-100` so their class_name
	# registrations land before downstream consumers parse.
	var enabled_entries: Array[Dictionary] = []
	for mod_id in cfg.get_section_keys(section):
		var entry: Variant = cfg.get_value(section, mod_id)
		if typeof(entry) != TYPE_DICTIONARY:
			push_warning("[Lib] %s: malformed entry (expected dict), skipping" % mod_id)
			continue
		if not entry.get("enabled", false):
			continue
		var pinned_version: String = entry.get("version", "")
		var meta := _read_mod_priority(mod_index, roots, mod_id)
		enabled_entries.append({
			"id": mod_id,
			"priority": meta.get("priority", 0),
			"mod_name": meta.get("mod_name", mod_id),
			"file_name": meta.get("file_name", mod_id),
			"pinned_version": pinned_version,
		})

	enabled_entries.sort_custom(_compare_load_order)

	var loaded := 0
	var skipped := 0
	for ent in enabled_entries:
		var mod_id: String = ent["id"]
		var pinned_version: String = ent["pinned_version"]
		if _try_load_mod_indexed(mod_index, mod_id, pinned_version):
			loaded += 1
			continue
		# Index miss: launcher hasn't refreshed since this mod was
		# enabled, OR the archive moved on disk. Fall back to a
		# targeted scan that opens archives only until it finds
		# THIS specific id and stops. Disabled-mod archives are
		# never opened on this path either; the fallback only
		# triggers for enabled ids whose entry was missing.
		if _try_load_mod_from_roots(roots, mod_id, pinned_version):
			loaded += 1
		else:
			skipped += 1

	print("[Lib] mods: profile=%s loaded=%d skipped=%d (roots=%d)" % [
		profile_name, loaded, skipped, roots.size(),
	])


## Mirror of vostok-mod-loader's `_compare_load_order`. Lower priority
## runs first; tiebreak by lowercase mod_name then lowercase file_name.
func _compare_load_order(a: Dictionary, b: Dictionary) -> bool:
	if a["priority"] != b["priority"]:
		return int(a["priority"]) < int(b["priority"])
	var a_name := (a["mod_name"] as String).to_lower()
	var b_name := (b["mod_name"] as String).to_lower()
	if a_name != b_name:
		return a_name < b_name
	return (a["file_name"] as String).to_lower() < (b["file_name"] as String).to_lower()


## Read priority + mod_name + file_name for `mod_id` from mod_index.cfg.
## Falls back to a targeted scan when the id isn't in the index.
func _read_mod_priority(mod_index: Dictionary, roots: Array[String], mod_id: String) -> Dictionary:
	if mod_index.has(mod_id):
		var entry: Dictionary = mod_index[mod_id]
		var path: String = entry.get("path", "")
		return {
			"priority": int(entry.get("priority", 0)),
			"mod_name": String(entry.get("name", mod_id)),
			"file_name": path.get_file() if not path.is_empty() else mod_id,
		}
	for root in roots:
		var hit := _find_mod_in_root(root, mod_id)
		if not hit.is_empty():
			var fmanifest: ConfigFile = hit["manifest"]
			return {
				"priority": int(fmanifest.get_value("mod", "priority", 0)),
				"mod_name": String(fmanifest.get_value("mod", "name", mod_id)),
				"file_name": (hit["path"] as String).get_file(),
			}
	return { "priority": 0, "mod_name": mod_id, "file_name": mod_id }


## Read `<game-dir>/.crabby/mod_index.cfg` into a flat map.
func _load_mod_index(game_dir: String) -> Dictionary:
	var path := game_dir.path_join(".crabby/mod_index.cfg")
	if not FileAccess.file_exists(path):
		return {}
	var cfg := ConfigFile.new()
	var err := cfg.load(path)
	if err != OK:
		push_warning("[Lib] mod_index.cfg parse failed at %s (err=%d), falling back to scan" % [path, err])
		return {}
	var schema: int = cfg.get_value("crabby", "schema_version", -1)
	if schema != 1:
		push_warning("[Lib] mod_index.cfg schema_version=%d (expected 1), falling back to scan" % schema)
		return {}
	var out: Dictionary = {}
	for sec in cfg.get_sections():
		if not (sec as String).begins_with("mod."):
			continue
		var id := (sec as String).substr(4)
		var p: String = cfg.get_value(sec, "path", "")
		if p.is_empty():
			continue
		out[id] = {
			"path": p,
			"source": cfg.get_value(sec, "source", "vmz"),
			"version": cfg.get_value(sec, "version", ""),
			"mtime": int(cfg.get_value(sec, "mtime", 0)),
			"name": String(cfg.get_value(sec, "name", id)),
			"priority": int(cfg.get_value(sec, "priority", 0)),
			"cache_path": String(cfg.get_value(sec, "cache_path", "")),
		}
	return out


## Look up `mod_id` in the launcher-written mod index. On hit, mount
## the referenced archive directly without scanning.
func _try_load_mod_indexed(mod_index: Dictionary, mod_id: String, pinned_version: String) -> bool:
	if not mod_index.has(mod_id):
		return false
	var entry: Dictionary = mod_index[mod_id]
	var path: String = entry.get("path", "")
	var source: String = entry.get("source", "vmz")
	if path.is_empty():
		return false
	if source == "folder":
		var mod_txt := path.path_join("mod.txt")
		if not FileAccess.file_exists(mod_txt):
			return false
		var raw: String
		var f := FileAccess.open(mod_txt, FileAccess.READ)
		if f == null:
			return false
		raw = f.get_as_text()
		f.close()
		if raw.begins_with("﻿"):
			raw = raw.substr(1)
		var fmanifest := ConfigFile.new()
		if fmanifest.parse(_quote_unquoted_hooks_values(raw)) != OK:
			return false
		return _mount_and_instantiate(mod_id, pinned_version, path, fmanifest, true, "")
	if not FileAccess.file_exists(path):
		return false
	var manifest := _read_mod_txt(path)
	if manifest == null:
		return false
	if manifest.get_value("mod", "id", "") != mod_id:
		return false
	var cache_path: String = String(entry.get("cache_path", ""))
	return _mount_and_instantiate(mod_id, pinned_version, path, manifest, false, cache_path)


## Walk `roots` in order; for each, look for `mod_id` (vmz/zip archive
## or folder mod). First hit wins.
func _try_load_mod_from_roots(roots: Array[String], mod_id: String, pinned_version: String) -> bool:
	for root in roots:
		var hit := _find_mod_in_root(root, mod_id)
		if not hit.is_empty():
			return _mount_and_instantiate(mod_id, pinned_version, hit["path"], hit["manifest"], hit["is_folder"], "")
	push_warning("[Lib] %s: no archive or folder in any root has this id, skipping" % mod_id)
	return false


## Probe a single root for `mod_id`. Returns a dict
## `{ path, manifest, is_folder }` on hit, or empty dict on miss.
func _find_mod_in_root(root_path: String, mod_id: String) -> Dictionary:
	var dir := DirAccess.open(root_path)
	if dir == null:
		return {}

	for fname in dir.get_files():
		var lower := (fname as String).to_lower()
		if not (lower.ends_with(".vmz") or lower.ends_with(".zip")):
			continue
		var path := root_path.path_join(fname)
		var manifest := _read_mod_txt(path)
		if manifest == null:
			continue
		if manifest.get_value("mod", "id", "") == mod_id:
			return { "path": path, "manifest": manifest, "is_folder": false }

	for sub in dir.get_directories():
		var folder := root_path.path_join(sub)
		var mod_txt := folder.path_join("mod.txt")
		if not FileAccess.file_exists(mod_txt):
			continue
		var f := FileAccess.open(mod_txt, FileAccess.READ)
		if f == null:
			push_warning("[Lib] folder mod.txt unreadable: %s, skipping" % mod_txt)
			continue
		var raw := f.get_as_text()
		f.close()
		if raw.begins_with("﻿"):
			raw = raw.substr(1)
		var fmanifest := ConfigFile.new()
		var ferr := fmanifest.parse(_quote_unquoted_hooks_values(raw))
		if ferr != OK:
			push_warning("[Lib] mod.txt parse failed for folder mod at %s (err=%d), skipping" % [mod_txt, ferr])
			continue
		if fmanifest.get_value("mod", "id", "") == mod_id:
			return { "path": folder, "manifest": fmanifest, "is_folder": true }

	return {}


## Common mount+instantiate path for both archives and folders.
## `cache_path` (empty string when none): absolute path to the launcher-
## written pre-rewritten archive. When set and the file exists, mount
## directly, skipping the in-place vmz_to_zip_cache rewrite.
func _mount_and_instantiate(
	mod_id: String,
	pinned_version: String,
	matched_path: String,
	matched_manifest: ConfigFile,
	is_folder: bool,
	cache_path: String,
) -> bool:
	var disk_version: String = matched_manifest.get_value("mod", "version", "")
	if not pinned_version.is_empty() and disk_version != pinned_version:
		push_warning("[Lib] %s: version drift (config v%s, archive v%s), loading archive version" % [
			mod_id, pinned_version, disk_version,
		])

	if is_folder:
		var zip_prefix := _compute_folder_zip_prefix(matched_path, matched_manifest)
		var zipped := _folder_to_zip_cache(mod_id, matched_path, zip_prefix)
		if zipped.is_empty():
			push_error("[Lib] %s: folder→zip cache failed for %s" % [mod_id, matched_path])
			return false
		if not ProjectSettings.load_resource_pack(zipped, true):
			push_error("[Lib] %s: load_resource_pack failed for cached zip %s" % [mod_id, zipped])
			return false
		if _DIAG:
			print("[Lib DIAG] %s: load_resource_pack OK (folder zipped to %s)" % [mod_id, zipped])
		var instantiated := _instantiate_mod_autoloads(mod_id, matched_manifest)
		print("[Lib] mod loaded (folder): %s v%s (%d autoload(s)) from %s [prefix=%s]" % [
			mod_id, disk_version, instantiated, matched_path, zip_prefix,
		])
		return true

	# Fast path: launcher pre-rewrote the archive into a ready-to-mount
	# zip at `cache_path`. Skip the runtime in-place rewrite entirely.
	# Any failure here falls through to the slow path so a stale /
	# corrupt cache doesn't lock the mod out.
	var mount_path := matched_path
	if not cache_path.is_empty() and FileAccess.file_exists(cache_path):
		if ProjectSettings.load_resource_pack(cache_path, true):
			mount_path = cache_path
			if _DIAG:
				print("[Lib DIAG] %s: mounted pre-rewritten cache %s" % [mod_id, cache_path])
		else:
			push_warning("[Lib] %s: cache_path %s failed to mount; falling back to in-place rewrite" % [mod_id, cache_path])
	# Slow path: try the source archive directly, then the in-place
	# rewrite cache. Reached when there's no cache_path, the cached
	# file doesn't exist (mod just enabled and async cache hasn't
	# completed), or the cached mount failed.
	if mount_path == matched_path and not ProjectSettings.load_resource_pack(mount_path, true):
		if mount_path.get_extension().to_lower() == "vmz":
			var alt := _vmz_to_zip_cache(mount_path)
			if not alt.is_empty() and ProjectSettings.load_resource_pack(alt, true):
				mount_path = alt
			else:
				push_error("[Lib] %s: load_resource_pack failed for %s (and vmz->zip fallback)" % [mod_id, matched_path])
				return false
		else:
			push_error("[Lib] %s: load_resource_pack failed for %s" % [mod_id, matched_path])
			return false

	if _DIAG:
		print("[Lib DIAG] %s: load_resource_pack OK (mounted %s)" % [mod_id, mount_path])
	var instantiated_arch := _instantiate_mod_autoloads(mod_id, matched_manifest)
	print("[Lib] mod loaded: %s v%s (%d autoload(s)) from %s" % [mod_id, disk_version, instantiated_arch, mount_path])
	return true


## Copy `vmz_path` to `user://crabby_vmz_cache/<basename>.zip` and return
## the cached path. Fallback path used when the launcher's pre-rewritten
## cache is missing (mod just-enabled, async cache not yet caught up).
func _vmz_to_zip_cache(vmz_path: String) -> String:
	var cache_dir := ProjectSettings.globalize_path("user://crabby_vmz_cache")
	if not DirAccess.dir_exists_absolute(cache_dir):
		DirAccess.make_dir_recursive_absolute(cache_dir)
	var zip_name := vmz_path.get_file().get_basename() + ".v2.zip"
	var zip_path := cache_dir.path_join(zip_name)

	if FileAccess.file_exists(zip_path):
		var src_mtime := FileAccess.get_modified_time(vmz_path)
		var dst_mtime := FileAccess.get_modified_time(zip_path)
		if src_mtime <= dst_mtime:
			return zip_path

	var zr := ZIPReader.new()
	if zr.open(vmz_path) != OK:
		return ""
	var packer := ZIPPacker.new()
	if packer.open(zip_path) != OK:
		zr.close()
		return ""
	for entry in zr.get_files():
		var bytes := zr.read_file(entry)
		if entry.to_lower().ends_with(".gd"):
			var text := bytes.get_string_from_utf8()
			var rewritten := _rewrite_const_resource_preloads(text)
			if rewritten != text:
				bytes = rewritten.to_utf8_buffer()
		if packer.start_file(entry) == OK:
			packer.write_file(bytes)
			packer.close_file()
	packer.close()
	zr.close()
	return zip_path


## Rewrite `const NAME [:= | =] preload("...tres")` → `var NAME = preload(...)`
## so the binding is a live reference. Mirror of crabby-config::mod_cache's
## Rust implementation; lives here as the runtime fallback when the
## launcher's pre-rewritten cache is missing.
func _rewrite_const_resource_preloads(text: String) -> String:
	var lines := text.split("\n")
	var changed := false
	for i in lines.size():
		var line: String = lines[i]
		if not line.begins_with("const "):
			continue
		if line.find("preload(") < 0:
			continue
		var tres_idx := line.find(".tres\"")
		if tres_idx < 0:
			continue
		var rest: String = line.substr(6)
		var new_line := "var " + rest.replace(":=", "=")
		lines[i] = new_line
		changed = true
	if not changed:
		return text
	return "\n".join(lines)


## Determine the prefix to use when zipping a folder mod, so that the
## resulting zip's internal layout matches the autoload's res:// paths.
func _compute_folder_zip_prefix(folder_path: String, manifest: ConfigFile) -> String:
	if not manifest.has_section("autoload"):
		return ""
	var first_path := ""
	for autoload_name in manifest.get_section_keys("autoload"):
		var raw: String = manifest.get_value("autoload", autoload_name, "")
		if raw.is_empty():
			continue
		if raw.begins_with("*"):
			raw = raw.substr(1)
		if raw.begins_with("res://"):
			raw = raw.substr(6)
		first_path = raw
		break
	if first_path.is_empty():
		return ""
	var basename := first_path.get_file()
	var found := _find_first_named(folder_path, basename, "")
	if found.is_empty():
		return ""
	var want_dir := first_path.substr(0, first_path.length() - basename.length())
	var have_dir := found.substr(0, found.length() - basename.length())
	if want_dir == have_dir:
		return ""
	if have_dir.is_empty():
		return want_dir.trim_suffix("/")
	push_warning("[Lib] folder layout mismatch: want %s, have %s, prepending %s" % [
		want_dir, have_dir, want_dir,
	])
	return want_dir.trim_suffix("/")


## Recursively search `dir` for a file named `name` and return its
## relative path (forward slashes). Returns "" if not found.
func _find_first_named(dir_path: String, name: String, rel_prefix: String) -> String:
	var dir := DirAccess.open(dir_path)
	if dir == null:
		return ""
	for fname in dir.get_files():
		if fname == name:
			return rel_prefix + fname
	for sub in dir.get_directories():
		var next := (sub + "/") if rel_prefix.is_empty() else rel_prefix + sub + "/"
		var hit := _find_first_named(dir_path.path_join(sub), name, next)
		if not hit.is_empty():
			return hit
	return ""


## Zip an unpacked folder mod into `user://crabby_vmz_cache/<mod_id>.zip`
## so it can be mounted via load_resource_pack.
func _folder_to_zip_cache(mod_id: String, folder_path: String, zip_prefix: String) -> String:
	var cache_dir := ProjectSettings.globalize_path("user://crabby_vmz_cache")
	if not DirAccess.dir_exists_absolute(cache_dir):
		DirAccess.make_dir_recursive_absolute(cache_dir)
	var safe_id := mod_id.replace("/", "_").replace(":", "_").replace("\\", "_")
	var zip_path := cache_dir.path_join(safe_id + ".zip")

	if FileAccess.file_exists(zip_path):
		DirAccess.remove_absolute(zip_path)

	var packer := ZIPPacker.new()
	if packer.open(zip_path) != OK:
		return ""
	var initial_prefix := "" if zip_prefix.is_empty() else (zip_prefix + "/")
	if not _zip_walk(packer, folder_path, initial_prefix):
		packer.close()
		return ""
	packer.close()
	return zip_path


## Recursively walk `abs_dir` and add every file to `packer` under
## `rel_prefix`. Same const→var rewrite as `_vmz_to_zip_cache`.
func _zip_walk(packer: ZIPPacker, abs_dir: String, rel_prefix: String) -> bool:
	var dir := DirAccess.open(abs_dir)
	if dir == null:
		return false
	for fname in dir.get_files():
		var src_path := abs_dir.path_join(fname)
		var f := FileAccess.open(src_path, FileAccess.READ)
		if f == null:
			push_warning("[Lib] zip: skipping unreadable %s" % src_path)
			continue
		var rel: String = (rel_prefix + fname) if rel_prefix.is_empty() else rel_prefix + fname
		if packer.start_file(rel) != OK:
			f.close()
			continue
		var bytes := f.get_buffer(f.get_length())
		if fname.to_lower().ends_with(".gd"):
			var text := bytes.get_string_from_utf8()
			var rewritten := _rewrite_const_resource_preloads(text)
			if rewritten != text:
				bytes = rewritten.to_utf8_buffer()
		packer.write_file(bytes)
		packer.close_file()
		f.close()
	for sub in dir.get_directories():
		var next_prefix := (sub + "/") if rel_prefix.is_empty() else rel_prefix + sub + "/"
		if not _zip_walk(packer, abs_dir.path_join(sub), next_prefix):
			return false
	return true


## Read mod.txt from inside an archive and return it as a ConfigFile.
## Pre-processes the `[hooks]` section to tolerate vostok's
## wiki-documented unquoted form.
func _read_mod_txt(archive_path: String) -> ConfigFile:
	var zr := ZIPReader.new()
	var oerr := zr.open(archive_path)
	if oerr != OK:
		push_warning("[Lib] archive open failed for %s (err=%d), skipping" % [archive_path, oerr])
		return null
	if not zr.file_exists("mod.txt"):
		zr.close()
		push_warning("[Lib] archive missing mod.txt: %s, skipping" % archive_path)
		return null
	var bytes := zr.read_file("mod.txt")
	zr.close()
	var text := bytes.get_string_from_utf8()
	if text.begins_with("﻿"):
		text = text.substr(1)
	var preprocessed := _quote_unquoted_hooks_values(text)
	var cfg := ConfigFile.new()
	var perr := cfg.parse(preprocessed)
	if perr != OK:
		var preview := preprocessed.substr(0, 400).replace("\n", "\\n")
		push_warning("[Lib] mod.txt parse failed in archive %s (err=%d). preview=%s" % [
			archive_path, perr, preview,
		])
		return null
	return cfg


## Quote the values of unquoted entries inside `[hooks]` sections.
## Mirrors vostok-mod-loader's `_quote_unquoted_hooks_values` exactly
## so the two loaders agree on what counts as a parseable manifest.
func _quote_unquoted_hooks_values(text: String) -> String:
	var lines := text.split("\n")
	var out := PackedStringArray()
	var in_hooks := false
	for line in lines:
		var stripped := line.strip_edges()
		if stripped.begins_with("[") and stripped.ends_with("]"):
			in_hooks = stripped.to_lower() == "[hooks]"
			out.append(line)
			continue
		if not in_hooks:
			out.append(line)
			continue
		if stripped.is_empty() or stripped.begins_with("#") or stripped.begins_with(";"):
			out.append(line)
			continue
		var eq_pos := line.find("=")
		if eq_pos < 0:
			out.append(line)
			continue
		var key_part := line.substr(0, eq_pos)
		var val_part := line.substr(eq_pos + 1)
		if val_part.strip_edges(true, false).begins_with("\""):
			out.append(line)
			continue
		var comment := ""
		var comment_pos := -1
		for j in val_part.length():
			var ch := val_part[j]
			if ch == "#" or ch == ";":
				comment_pos = j
				break
		if comment_pos >= 0:
			comment = val_part.substr(comment_pos)
			val_part = val_part.substr(0, comment_pos)
		var val_trim := val_part.strip_edges()
		var escaped := val_trim.replace("\\", "\\\\").replace("\"", "\\\"")
		var rebuilt := "%s= \"%s\"" % [key_part, escaped]
		if not comment.is_empty():
			rebuilt += "  " + comment
		out.append(rebuilt)
	return "\n".join(out)


## For each entry under `[autoload]` in the mod's manifest, load the
## referenced script/scene and add it as a child of /root.
##
## Autoload path prefixes (any combination, any order, stripped before
## load):
##   `*`  Godot's own "instantiate as a Node singleton" marker. We
##        already instantiate every mod autoload as a Node, so it's a
##        no-op for us beyond needing to be stripped off the path.
##   `!`  Metro / Vostok Mod Loader's `autoload_prepend` marker: the
##        author wants this autoload registered BEFORE other autoloads.
##        Crabby instantiates mod autoloads via deferred `add_child`
##        on the next idle frame (alongside vanilla autoloads), so the
##        absolute "before vanilla" guarantee can't be honored, but we
##        DO process `!`-prefixed entries first within the mod so they
##        win against the mod's own non-prefixed autoloads. Without
##        stripping the `!`, `load("!res://...")` mangles the path
##        ("!res:/...") and the autoload silently fails to load.
func _instantiate_mod_autoloads(mod_id: String, manifest: ConfigFile) -> int:
	if not manifest.has_section("autoload"):
		return 0
	# Two passes: prepend-marked autoloads first, then the rest. Order
	# within each pass follows manifest key order.
	var prepend_entries: Array = []
	var normal_entries: Array = []
	for autoload_name in manifest.get_section_keys("autoload"):
		var raw_path: String = manifest.get_value("autoload", autoload_name, "")
		if raw_path.is_empty():
			continue
		# Strip a run of leading `*` / `!` prefix chars in any order.
		var res_path := raw_path
		var is_prepend := false
		while res_path.begins_with("*") or res_path.begins_with("!"):
			if res_path.begins_with("!"):
				is_prepend = true
			res_path = res_path.substr(1)
		if is_prepend:
			prepend_entries.append([autoload_name, res_path])
		else:
			normal_entries.append([autoload_name, res_path])
	var count := 0
	for e in prepend_entries:
		if _instantiate_one_autoload(mod_id, e[0], e[1]):
			count += 1
	for e in normal_entries:
		if _instantiate_one_autoload(mod_id, e[0], e[1]):
			count += 1
	return count


func _instantiate_one_autoload(mod_id: String, autoload_name: String, res_path: String) -> bool:
	var resource: Resource = load(res_path)
	if resource == null:
		push_error("[Lib] %s: autoload %s failed to load %s" % [mod_id, autoload_name, res_path])
		return false

	if get_tree().root.has_node(autoload_name):
		push_warning("[Lib] %s: autoload name %s already in tree, Godot will rename" % [mod_id, autoload_name])

	if resource is PackedScene:
		var inst: Node = (resource as PackedScene).instantiate()
		if inst == null:
			push_error("[Lib] %s: PackedScene.instantiate returned null for %s" % [mod_id, autoload_name])
			return false
		inst.name = autoload_name
		# Defer the add. At this point the engine is mid-way through the
		# autoload chain and /root is "busy setting up children", so a
		# direct add_child errors. Deferring runs the add on the next
		# idle frame, which is also when vanilla autoloads land.
		get_tree().root.add_child.call_deferred(inst)
		return true

	if resource is GDScript:
		var gds := resource as GDScript
		if not gds.can_instantiate():
			push_error("[Lib] %s: autoload %s script can't instantiate at %s (parse error?)" % [mod_id, autoload_name, res_path])
			return false
		var inst: Variant = gds.new()
		if inst == null:
			push_warning("[Lib] %s: autoload %s .new() returned null at %s" % [mod_id, autoload_name, res_path])
			return false
		if not (inst is Node):
			push_warning("[Lib] %s: autoload %s is not a Node at %s, not added to tree" % [mod_id, autoload_name, res_path])
			return false
		(inst as Node).name = autoload_name
		get_tree().root.add_child.call_deferred(inst as Node)
		return true

	push_warning("[Lib] %s: autoload %s is neither PackedScene nor GDScript" % [mod_id, autoload_name])
	return false
