class_name DocumentController
extends Node
## The open sketch's file: new, open (picker, Recents, other apps), save with preview,
## autosave, and the Recents list shown on the home screen. Never organizes files — that's
## the OS's job.

signal opened  ## a document is ready on the canvas
signal status(message: String)  ## for the canvas info line
signal home_message(message: String)  ## problems to show on the home screen

const AUTOSAVE_SECONDS := 15.0

var canvas: SketchCanvas
var app_files: AppFiles
var thumbnails: ThumbnailRenderer
## Copies for the iOS export picker are written here first, then removed.
const COPY_DIR := "user://copies"

## The temporary copy the export picker is showing, if any.
var _pending_copy := ""
## Where + New puts sketches (absolute).
var sketch_dir := ""
## The open sketch's file (absolute) and, for picked files, its access bookmark.
var doc_path := ""
var doc_bookmark := ""
## True while the canvas is shown, so autosave runs only then.
var active := false

var _saving := false


func _init(p_canvas: SketchCanvas, p_app_files: AppFiles, p_thumbnails: ThumbnailRenderer) -> void:
	canvas = p_canvas
	app_files = p_app_files
	thumbnails = p_thumbnails
	app_files.file_picked.connect(func(path: String, bookmark: String) -> void: open_path(path, bookmark))
	app_files.file_exported.connect(func(name: String) -> void:
		_drop_pending_copy()
		status.emit("Saved a copy without history: " + name))
	app_files.export_cancelled.connect(_drop_pending_copy)


func _ready() -> void:
	var autosave := Timer.new()
	autosave.wait_time = AUTOSAVE_SECONDS
	autosave.timeout.connect(func() -> void:
		if active and canvas.is_dirty():
			save_current())
	add_child(autosave)
	autosave.start()


func new_sketch() -> void:
	await save_current()
	close()
	canvas.new_document()
	DirAccess.make_dir_recursive_absolute(sketch_dir)
	var n := 1
	while FileAccess.file_exists(_sketch_path(n)):
		n += 1
	doc_path = _sketch_path(n)
	opened.emit()


func open_path(path: String, bookmark: String) -> bool:
	await save_current()
	var err: String = canvas.load_document(path)
	if not err.is_empty():
		home_message.emit("Couldn't open %s: %s" % [path.get_file(), err])
		if not bookmark.is_empty():
			app_files.release_file(path)
		return false
	close()
	doc_path = path
	doc_bookmark = bookmark
	app_files.touch_recent(AppPaths.store(path), bookmark)
	opened.emit()
	return true


func open_recent(entry: Dictionary) -> void:
	var path := resolve(entry)
	if path.is_empty():
		home_message.emit("%s can't be found. It may have been moved or deleted." % entry.name)
		return
	open_path(path, entry.bookmark)


## Saves the open sketch in place (with a fresh preview), only if it was edited: a file
## that was only viewed is never rewritten (docs/ARCHITECTURE.md §3h). A new sketch that was
## never drawn in isn't written, so + New then back leaves no empty files behind.
func save_current() -> void:
	if doc_path.is_empty() or _saving or not canvas.is_dirty():
		return
	if canvas.stroke_count() == 0 and not FileAccess.file_exists(doc_path):
		return
	_saving = true
	await update_thumbnail()
	var err: String = canvas.save_document(doc_path)
	_saving = false
	if err.is_empty():
		app_files.touch_recent(AppPaths.store(doc_path), doc_bookmark)
		status.emit("")
	else:
		status.emit("Save failed: " + err)


## Renders the preview embedded in the file on the next save, from a fixed angle: the same
## sketch gets the same preview whoever saves it and however they were looking at it.
func update_thumbnail() -> void:
	var png: PackedByteArray = await thumbnails.render(canvas)
	var err: String = canvas.set_thumbnail(png)
	if not err.is_empty():
		push_warning("Thumbnail skipped: " + err)


## Forgets the open file (after saving it), ending any security-scoped access.
func close() -> void:
	if not doc_bookmark.is_empty():
		app_files.release_file(doc_path)
	doc_path = ""
	doc_bookmark = ""


## System "Open" picker: native on iOS, Godot's native dialog (or its own) elsewhere.
func show_open_dialog(dialog_parent: Node) -> void:
	if app_files.pick_sketch():
		return  # Result arrives through file_picked.
	DirAccess.make_dir_recursive_absolute(sketch_dir)
	if DisplayServer.has_feature(DisplayServer.FEATURE_NATIVE_DIALOG_FILE):
		DisplayServer.file_dialog_show("Open Sketch", sketch_dir, "", false, DisplayServer.FILE_DIALOG_MODE_OPEN_FILE,
			PackedStringArray(["*.airdraw;AirDraw Sketch"]),
			func(ok: bool, paths: PackedStringArray, _filter: int) -> void:
				if ok and not paths.is_empty():
					open_path(paths[0], ""))
	else:
		var dialog := FileDialog.new()
		dialog.file_mode = FileDialog.FILE_MODE_OPEN_FILE
		dialog.access = FileDialog.ACCESS_FILESYSTEM
		dialog.filters = PackedStringArray(["*.airdraw;AirDraw Sketch"])
		dialog.current_dir = sketch_dir
		dialog.file_selected.connect(func(p: String) -> void: open_path(p, ""))
		dialog.close_requested.connect(dialog.queue_free)
		dialog_parent.add_child(dialog)
		dialog.popup_centered_ratio(0.7)


## ⋯ › Save a Copy Without History…: the sketch as it is now, minus its undo history, so
## strokes deleted earlier can't be brought back by whoever gets it. The open sketch keeps
## its history. iOS: written to a temporary file that the system "Save to Files" picker
## copies where the user chooses. Elsewhere: the native save dialog (or Godot's own).
func save_copy_without_history(dialog_parent: Node) -> void:
	await update_thumbnail()
	var title := "Save a Copy Without History"
	var name := _copy_name()
	DirAccess.make_dir_recursive_absolute(COPY_DIR)
	_pending_copy = ProjectSettings.globalize_path(COPY_DIR.path_join(name))
	var err: String = canvas.save_copy_without_history(_pending_copy)
	if not err.is_empty():
		_drop_pending_copy()
		status.emit("Couldn't save a copy: " + err)
		return
	if app_files.export_file(_pending_copy):
		return  # Result arrives through file_exported / export_cancelled.
	_drop_pending_copy()
	var save_to := func(path: String) -> void:
		if not path.ends_with(".airdraw"):
			path += ".airdraw"
		var e: String = canvas.save_copy_without_history(path)
		status.emit("Saved a copy without history: " + path.get_file() if e.is_empty() else "Couldn't save a copy: " + e)
	if DisplayServer.has_feature(DisplayServer.FEATURE_NATIVE_DIALOG_FILE):
		DisplayServer.file_dialog_show(title, sketch_dir, name, false, DisplayServer.FILE_DIALOG_MODE_SAVE_FILE,
			PackedStringArray(["*.airdraw;AirDraw Sketch"]),
			func(ok: bool, paths: PackedStringArray, _filter: int) -> void:
				if ok and not paths.is_empty():
					save_to.call(paths[0]))
	else:
		var dialog := FileDialog.new()
		dialog.title = title
		dialog.file_mode = FileDialog.FILE_MODE_SAVE_FILE
		dialog.access = FileDialog.ACCESS_FILESYSTEM
		dialog.filters = PackedStringArray(["*.airdraw;AirDraw Sketch"])
		dialog.current_dir = sketch_dir
		dialog.current_file = name
		dialog.file_selected.connect(save_to)
		dialog.close_requested.connect(dialog.queue_free)
		dialog_parent.add_child(dialog)
		dialog.popup_centered_ratio(0.7)


## "Sketch 3 copy.airdraw" for the open sketch (the picker lets the user rename it).
func _copy_name() -> String:
	var base := doc_path.get_file().get_basename() if not doc_path.is_empty() else "Sketch"
	return base + " copy.airdraw"


func _drop_pending_copy() -> void:
	if not _pending_copy.is_empty():
		DirAccess.remove_absolute(_pending_copy)
		_pending_copy = ""


## Home screen cards: [{entry, texture, exists}], newest first.
func recent_items() -> Array:
	var items := []
	for entry in app_files.recents():
		var path := resolve(entry)
		var texture: Texture2D = null
		if not path.is_empty():
			var png: PackedByteArray = canvas.thumbnail_of(path)
			var img := Image.new()
			if not png.is_empty() and img.load_png_from_buffer(png) == OK:
				texture = ImageTexture.create_from_image(img)
			if not entry.bookmark.is_empty():
				app_files.release_file(path)
		items.append({"entry": entry, "texture": texture, "exists": not path.is_empty()})
	return items


func remove_recent(entry: Dictionary) -> void:
	app_files.remove_recent(entry.path)


func reveal(entry: Dictionary) -> void:
	var path := resolve(entry)
	if path.is_empty():
		home_message.emit("%s can't be found." % entry.name)
		return
	if OS.has_feature("ios"):
		OS.shell_open("shareddocuments://" + path.replace(" ", "%20"))
	else:
		OS.shell_show_in_file_manager(path)
	if not entry.bookmark.is_empty():
		app_files.release_file(path)


## A Recents entry's current absolute path ("" if gone). Bookmarked entries gain access
## that the caller releases.
func resolve(entry: Dictionary) -> String:
	if not entry.bookmark.is_empty():
		return app_files.open_bookmark(entry.bookmark)
	var path := AppPaths.resolve(entry.path)
	return path if FileAccess.file_exists(path) else ""


func _sketch_path(n: int) -> String:
	return sketch_dir.path_join("Sketch %d.airdraw" % n)
