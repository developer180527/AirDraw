extends Node3D
## App root: assembles the scene and UI, and switches between the home screen (launcher) and
## the canvas. Files live in DocumentController, the drawing guide in GuideController;
## pointer input goes through the Rust SketchSession (platform input → router → canvas).

## Exporter plugin libraries (desktop). iOS builds link plugins in statically.
const PLUGIN_DIR := "user://plugins"
const GUIDE_READY := "Guide ready. Orbit to see it, then draw on it."

var rig: OrbitCamera
var environment: WorldEnvironment
var sun: DirectionalLight3D
var canvas: SketchCanvas
var session: SketchSession
var guides: GuideController
var documents: DocumentController
## The drawing screen's chrome (named for tests and older callers).
var toolbar: EditorShell
var export_dialog: ExportDialog
var brush_editor: BrushEditor
var thumbnails: ThumbnailRenderer
var home: HomeScreen
var app_files: AppFiles
## The pen's tool: "draw" (ink on the active guide) or "guide" (the next stroke draws a guide).
var tool := "draw"

## Overridable before the node enters the tree (tests).
var sketch_dir := ""
var recents_file := "user://recents.toml"
var status := ""

## The open sketch's file (absolute), "" on the home screen.
var doc_path: String:
	get:
		return documents.doc_path if documents else ""


func _ready() -> void:
	# scripts/bench_ipad.sh launches the app with these to run a benchmark on device.
	if "bench-live-stroke" in OS.get_cmdline_user_args():
		get_tree().change_scene_to_file.call_deferred("res://tests/live_stroke_bench.tscn")
		return
	if "bench-file-format" in OS.get_cmdline_user_args():
		get_tree().change_scene_to_file.call_deferred("res://tests/file_format_bench.tscn")
		return
	Input.use_accumulated_input = false
	# User-facing folders first (and the one-time iOS Documents migration, which moves user
	# brushes) so everything below finds files in their new places.
	app_files = AppFiles.new()
	add_child(app_files)
	AppPaths.setup(app_files)
	AppPaths.migrate_ios_documents()
	app_files.load_recents(ProjectSettings.globalize_path(recents_file))

	# Lighting comes from the sketch's Look (applied in _apply_look); these are its defaults.
	environment = WorldEnvironment.new()
	environment.environment = Environment.new()
	environment.environment.background_mode = Environment.BG_COLOR
	environment.environment.background_color = Color(0.93, 0.93, 0.91)
	environment.environment.ambient_light_source = Environment.AMBIENT_SOURCE_COLOR
	environment.environment.ambient_light_color = Color(0.75, 0.75, 0.8)
	add_child(environment)
	sun = DirectionalLight3D.new()
	sun.rotation = Vector3(-0.9, 0.5, 0.0)
	sun.light_energy = 0.8
	add_child(sun)
	rig = OrbitCamera.new()
	add_child(rig)

	canvas = SketchCanvas.new()
	canvas.load_brush_dir(ProjectSettings.globalize_path("user://brushes"))
	canvas.strokes_changed.connect(func(_n: int) -> void: _update_hud())
	canvas.brush_changed.connect(func(_id: String) -> void: _update_hud())
	canvas.look_changed.connect(_apply_look)
	add_child(canvas)

	session = SketchSession.new()
	session.canvas = canvas
	session.camera = rig.camera
	session.view_orbit.connect(rig.orbit)
	session.view_pan.connect(rig.pan)
	session.view_zoom.connect(rig.zoom)
	session.guide_nudged.connect(func(notches: float) -> void: guides.move(notches * 0.05))
	session.pen_action.connect(_pen_action)
	add_child(session)

	guides = GuideController.new(session)
	guides.changed.connect(func() -> void:
		if toolbar:
			toolbar.refresh_guide(guides.has_drawn(), guides.preset, guides.offset(), guides.opacity())
			if not guides.has_drawn():
				toolbar.dismiss_toast(GUIDE_READY)
		_update_hud())
	add_child(guides)

	thumbnails = ThumbnailRenderer.new()
	add_child(thumbnails)
	documents = DocumentController.new(canvas, app_files, thumbnails)
	documents.sketch_dir = ProjectSettings.globalize_path(sketch_dir) if not sketch_dir.is_empty() else AppPaths.sketches()
	documents.opened.connect(_show_canvas)
	documents.status.connect(_set_status)
	add_child(documents)

	toolbar = EditorShell.new()
	add_child(toolbar)
	toolbar.layout_changed.connect(_update_ui_regions)
	toolbar.plane_requested.connect(func(i: int) -> void:
		if i == GuideController.VIEW_ALIGNED:
			guides.align_to_view(rig.camera)
		else:
			guides.set_preset(i))
	toolbar.guide_offset_changed.connect(guides.set_offset)
	toolbar.guide_opacity_changed.connect(guides.set_opacity)
	toolbar.tool_selected.connect(set_tool)
	toolbar.look_requested.connect(func(id: String) -> void:
		canvas.set_look(id)
		_update_hud())
	session.guide_path_changed.connect(toolbar.show_guide_path)
	session.guide_drawn.connect(func(created: bool) -> void:
		if created:
			set_tool("draw")
			_set_status(GUIDE_READY)
		else:
			set_tool("guide")
			_set_status("Draw a longer line to make a guide."))
	session.pen_down.connect(func() -> void: toolbar.close_sheet())
	session.ui_toggle_requested.connect(func() -> void: toolbar.toggle_ui())
	toolbar.view_requested.connect(rig.set_view)
	toolbar.orbit_requested.connect(rig.orbit)
	toolbar.action.connect(_file_action)
	toolbar.setup(canvas, rig.camera)
	guides.attach(canvas)
	_apply_look()
	export_dialog = ExportDialog.new()
	export_dialog.finished.connect(_set_status)
	export_dialog.visibility_changed.connect(_update_ui_regions)
	toolbar.add_child(export_dialog)
	brush_editor = BrushEditor.new()
	brush_editor.status.connect(_set_status)
	brush_editor.visibility_changed.connect(_update_ui_regions)
	brush_editor.resized.connect(_update_ui_regions)
	toolbar.add_child(brush_editor)
	if OS.has_feature("pc"):
		DirAccess.make_dir_recursive_absolute(PLUGIN_DIR)
		var loaded: PackedStringArray = canvas.load_exporter_plugins(ProjectSettings.globalize_path(PLUGIN_DIR))
		if not loaded.is_empty():
			status = "\n".join(loaded)
	get_viewport().size_changed.connect(_update_ui_regions)

	var home_layer := CanvasLayer.new()
	home_layer.layer = 10
	add_child(home_layer)
	home = HomeScreen.new()
	home_layer.add_child(home)
	home.setup(toolbar.popup_theme())
	home.new_requested.connect(new_sketch)
	home.open_requested.connect(func() -> void: documents.show_open_dialog(home))
	home.recent_chosen.connect(documents.open_recent)
	home.remove_requested.connect(func(e: Dictionary) -> void:
		documents.remove_recent(e)
		home.show_recents(documents.recent_items()))
	home.reveal_requested.connect(documents.reveal)
	documents.home_message.connect(home.set_status)

	get_tree().set_auto_accept_quit(false)
	if OS.has_feature("macos"):
		var poll := Timer.new()
		poll.wait_time = 1.0
		poll.timeout.connect(_open_pending_files)
		add_child(poll)
		poll.start()
	guides.set_preset(0)
	# Opened from Finder / the command line with a sketch: go straight to it.
	for arg in OS.get_cmdline_args() + OS.get_cmdline_user_args():
		if arg.get_extension().to_lower() == "airdraw" and FileAccess.file_exists(arg):
			documents.open_path(arg, "")
			return
	show_home()


## macOS: sketches opened from Finder while AirDraw runs (queued by AirDraw's engine patch;
## the app is activated, and this also polls once a second as a fallback).
func _open_pending_files() -> void:
	if not ProjectSettings.has_meta("airdraw_open_files"):
		return
	var files: Array = ProjectSettings.get_meta("airdraw_open_files")
	ProjectSettings.remove_meta("airdraw_open_files")
	for f in files:
		if String(f).get_extension().to_lower() == "airdraw":
			documents.open_path(f, "")
			return


func _notification(what: int) -> void:
	if what == NOTIFICATION_APPLICATION_FOCUS_IN and documents:
		_open_pending_files()
	match what:
		NOTIFICATION_APPLICATION_PAUSED, NOTIFICATION_APPLICATION_FOCUS_OUT:
			if documents and documents.active and canvas.is_dirty():
				documents.save_current()
		NOTIFICATION_WM_CLOSE_REQUEST:
			if documents and documents.active:
				await documents.save_current()
			get_tree().quit()


# ---- Screens --------------------------------------------------------------------------

func show_home() -> void:
	await documents.save_current()
	documents.close()
	documents.active = false
	home.visible = true
	toolbar.visible = false
	export_dialog.hide()
	brush_editor.hide()
	home.show_recents(documents.recent_items())
	_update_ui_regions()


func _show_canvas() -> void:
	documents.active = true
	home.visible = false
	home.set_status("")
	toolbar.visible = true
	status = ""
	_update_ui_regions()
	_update_hud()
	toolbar.first_run_hint()


func new_sketch() -> void:
	await documents.new_sketch()


func update_thumbnail() -> void:
	await documents.update_thumbnail()


# ---- Canvas UI ------------------------------------------------------------------------

func _unhandled_input(event: InputEvent) -> void:
	if home and home.visible:
		return
	if event is InputEventKey and event.pressed and not event.echo:
		_handle_key(event)


func _handle_key(event: InputEventKey) -> void:
	var action := Shortcuts.action_for(event)
	match action:
		"tool_draw":
			set_tool("draw")
		"tool_guide":
			set_tool("draw" if tool == "guide" else "guide")
		"undo":
			canvas.undo()
		"redo":
			canvas.redo()
		"save":
			await documents.save_current()
			_set_status("Saved")
		"export", "clear":
			_file_action(action)
		"view_front":
			rig.set_view("front")
		"view_side":
			rig.set_view("side")
		"view_top":
			rig.set_view("top")
		"view_reset":
			rig.set_view("3d")
		"toggle_ui":
			toolbar.toggle_ui()
		"rebrush":
			if canvas.rebrush_last():
				_set_status("Re-brushed last stroke")
		_ when action.begins_with("brush_"):
			var id := toolbar.recent_brush(int(action.trim_prefix("brush_")) - 1)
			if not id.is_empty():
				canvas.set_brush(id)


## The sketch's Look sets the lighting: background, ambient, key light, shadows, and
## screen-space AO where the renderer has it (Forward+ on desktop, not the iPad's Mobile one).
func _apply_look() -> void:
	var e: Dictionary = canvas.look_environment()
	var env := environment.environment
	env.background_color = e.background
	env.ambient_light_color = e.ambient
	env.ambient_light_energy = e.ambient_energy
	sun.light_energy = e.sun_energy
	sun.visible = e.sun_energy > 0.0
	sun.shadow_enabled = e.shadows
	env.ssao_enabled = e.ssao and RenderingServer.get_current_rendering_method() == "forward_plus"
	# Compile the Look's shaders now, not during the first stroke (a visible stall on iPad).
	canvas.warm_up_look(rig.camera.global_transform)
	if toolbar:
		toolbar.show_look(canvas.look())


## Switches the pen's tool. The Guide tool draws one guide, then the app goes back to Draw.
func set_tool(t: String) -> void:
	tool = t
	session.guide_drawing = t == "guide"
	toolbar.set_tool(t)
	toolbar.set_guide_drawing(t == "guide")
	if t == "guide":
		_set_status("Draw a line: it becomes a surface you can draw on.")


func _file_action(action: String) -> void:
	match action:
		"gallery":
			show_home()
		"undo":
			canvas.undo()
		"redo":
			canvas.redo()
		"clear":
			canvas.clear()
			_set_status("Cleared — Undo brings it back")
		"remove_guide":
			canvas.remove_guide()
			_set_status("Guide removed — Undo brings it back")
		"export":
			export_dialog.open(canvas, toolbar.popup_theme())
		"save_copy":
			documents.save_copy_without_history(self)
		"edit_brush":
			brush_editor.open(canvas, canvas.current_brush(), toolbar.popup_theme(), toolbar.bar_rect().end.y)
		"import_brushes":
			var result: Dictionary = canvas.import_brushes(AppPaths.import_dir())
			var msg := "Imported %d brush(es)" % result.imported.size() if not result.imported.is_empty() else "No brushes in the import folder"
			for e in result.errors:
				msg += "\n⚠ " + e
			_set_status(msg)


## Native touch backends see touches that also hit the GUI; tell the session where UI is.
func _update_ui_regions() -> void:
	var screen := get_viewport().get_visible_rect()
	if home and home.visible:
		session.set_ui_regions([screen])
		return
	var regions: Array[Rect2] = toolbar.ui_rects()
	if export_dialog and export_dialog.visible:
		regions.append(screen)
	if brush_editor and brush_editor.visible:
		regions.append(brush_editor.get_global_rect())
	session.set_ui_regions(regions)


func _set_status(text: String) -> void:
	status = text
	if toolbar:
		toolbar.toast(text)
	_update_hud()


## Apple Pencil gestures. Placeholder mapping until there's an eraser and tool palette.
func _pen_action(action: String) -> void:
	match action:
		"doubletap":
			canvas.undo()
			_set_status("Pencil double-tap: undo")
		"squeeze":
			_cycle_brush(1)
			_set_status("Pencil squeeze: " + canvas.brush_name(canvas.current_brush()))


func _cycle_brush(step: int) -> void:
	var ids := canvas.brush_ids()
	var i := ids.find(canvas.current_brush())
	canvas.set_brush(ids[posmod(i + step, ids.size())])


func _update_hud() -> void:
	if toolbar and documents and canvas:
		toolbar.set_title(doc_path.get_file().get_basename(), canvas.is_dirty())
