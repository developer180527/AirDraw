class_name AppPaths
## Where user-facing files live: the app's Documents on iOS (Files › On My iPad › AirDraw),
## ~/Documents/AirDraw on desktop. Engine and app data (caches, settings, Recents, brushes)
## stay in user://, which AirDraw's engine patch keeps out of Documents on iOS.

## Base folder for user-facing files. Tests set `root_override` before setup.
static var root := ""
static var root_override := ""

## Recents stores files under `root` with this prefix: on iOS the container path changes
## with every reinstall, so absolute paths there would go stale.
const PREFIX := "airdraw-docs://"


static func setup(app_files: AppFiles) -> void:
	if not root_override.is_empty():
		root = ProjectSettings.globalize_path(root_override)
	else:
		var docs: String = app_files.documents_dir()
		root = docs if not docs.is_empty() else OS.get_system_dir(OS.SYSTEM_DIR_DOCUMENTS).path_join("AirDraw")
	for dir in [sketches(), exports(), import_dir()]:
		DirAccess.make_dir_recursive_absolute(dir)


static func sketches() -> String:
	return root.path_join("Sketches")


static func exports() -> String:
	return root.path_join("Exports")


static func import_dir() -> String:
	return root.path_join("Import")


## An app container's Documents folder on iOS, under any install (the ID changes with every
## reinstall) and either spelling of /var (the Files picker returns /private/var/…).
static var _ios_documents := RegEx.create_from_string("^(?:/private)?/var/mobile/Containers/Data/Application/[0-9A-Fa-f-]+/Documents/(.+)$")


## Absolute path → how Recents stores it: files in the app's Documents folder relative to
## it, however their path is spelled, so one file is always one entry.
static func store(path: String) -> String:
	for base in [root, "/private" + root, root.trim_prefix("/private")]:
		if not root.is_empty() and path.begins_with(base + "/"):
			return PREFIX + path.substr(base.length() + 1)
	var m := _ios_documents.search(path)
	if m and root.contains("/Containers/Data/Application/"):
		return PREFIX + m.get_string(1)
	return path


## Stored path → absolute path (also accepts legacy user:// entries).
static func resolve(stored: String) -> String:
	if stored.begins_with(PREFIX):
		return root.path_join(stored.substr(PREFIX.length()))
	return ProjectSettings.globalize_path(stored)


## iOS only, once: Documents used to be Godot's user:// folder. Moves AirDraw's files into
## the new layout and removes engine leftovers, so Files shows only Sketches, Exports, Import.
static func migrate_ios_documents() -> void:
	if not OS.has_feature("ios"):
		return
	var marker := "user://.documents_migrated"
	if FileAccess.file_exists(marker):
		return
	for engine_dir in ["shader_cache", "vulkan"]:
		_remove_tree(root.path_join(engine_dir))
	var old_sketch := root.path_join("documents/sketch.airdraw")
	if FileAccess.file_exists(old_sketch):
		DirAccess.rename_absolute(old_sketch, _free_name(sketches(), "Recovered Sketch"))
	_remove_tree(root.path_join("documents"))
	_merge_dir(root.path_join("exports"), exports())
	_merge_dir(root.path_join("import"), import_dir())
	_merge_dir(root.path_join("brushes"), ProjectSettings.globalize_path("user://brushes"))
	for f in ["recents.toml", "recents.toml.tmp"]:
		DirAccess.remove_absolute(root.path_join(f))
	var f := FileAccess.open(marker, FileAccess.WRITE)
	if f:
		f.store_string("1")


## Moves files from `from` into `to` and removes `from`. Safe when the two names differ only
## in case on a case-insensitive file system.
static func _merge_dir(from: String, to: String) -> void:
	if not DirAccess.dir_exists_absolute(from):
		return
	var tmp := from + ".migrating"
	if DirAccess.rename_absolute(from, tmp) != OK:
		return
	DirAccess.make_dir_recursive_absolute(to)
	for file in DirAccess.get_files_at(tmp):
		var target := to.path_join(file)
		if not FileAccess.file_exists(target):
			DirAccess.rename_absolute(tmp.path_join(file), target)
	_remove_tree(tmp)


static func _remove_tree(dir: String) -> void:
	if not DirAccess.dir_exists_absolute(dir):
		return
	for sub in DirAccess.get_directories_at(dir):
		_remove_tree(dir.path_join(sub))
	for file in DirAccess.get_files_at(dir):
		DirAccess.remove_absolute(dir.path_join(file))
	DirAccess.remove_absolute(dir)


static func _free_name(dir: String, base: String) -> String:
	var path := dir.path_join(base + ".airdraw")
	var n := 2
	while FileAccess.file_exists(path):
		path = dir.path_join("%s %d.airdraw" % [base, n])
		n += 1
	return path
