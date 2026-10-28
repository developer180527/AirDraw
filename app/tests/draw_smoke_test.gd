extends Node

const GLOW_BRUSH := """
schema = 1
id = "test.glow"
name = "Glow"

[[filters]]
type = "one_euro"

[[dynamics]]
type = "width"
base = 0.025

[geometry]
type = "tube"

[material]
type = "solid"
color = "#FFB000"
emission = 2.0
"""
## Feeds synthetic mouse strokes through the real input path, checks stroke count,
## saves a screenshot to user://smoke.png. Run: godot --path app res://tests/draw_smoke_test.tscn

func _ready() -> void:
	var main: Node3D = load("res://main.tscn").instantiate()
	# Isolated sketch folder and Recents, never the user's real ones.
	if DirAccess.dir_exists_absolute("user://test_sketches"):
		for f in DirAccess.get_files_at("user://test_sketches"):
			DirAccess.remove_absolute(ProjectSettings.globalize_path("user://test_sketches/" + f))
	DirAccess.remove_absolute(ProjectSettings.globalize_path("user://test_recents.toml"))
	main.set("sketch_dir", "user://test_sketches")
	AppPaths.root_override = "user://test_root"
	main.set("recents_file", "user://test_recents.toml")
	add_child(main)
	await get_tree().process_frame
	print("SMOKE starts_home=", main.get("home").visible)
	await main.new_sketch()
	await get_tree().process_frame
	var center := get_viewport().get_visible_rect().size * 0.5
	var canvas = main.get("canvas")
	if canvas == null:
		push_error("SMOKE main scene failed to initialize")
		get_tree().quit(1)
		return
	# A spiral on the front plane (Ink), then a loop on the top plane (Ribbon).
	await _stroke(center, func(t: float) -> Vector2: return Vector2(cos(t * 12.0), sin(t * 12.0)) * (40.0 + t * 220.0))
	_key(KEY_3)
	canvas.set_brush("com.airdraw.ribbon")
	await _stroke(center + Vector2(0, 120), func(t: float) -> Vector2: return Vector2(cos(t * TAU) * 260.0, sin(t * TAU) * 90.0))
	print("SMOKE strokes=", canvas.stroke_count())
	_key(KEY_Z, true)
	print("SMOKE after_undo=", canvas.stroke_count())
	_key(KEY_Z, true, true)
	print("SMOKE after_redo=", canvas.stroke_count())

	# Touch path (fallback backend): a tilted contact is a pen and draws (Marker)...
	canvas.set_brush("com.airdraw.marker")
	await _touch_stroke(0, center + Vector2(-300, -150), Vector2(250, 60), Vector2(0.4, 0.3), 0.6)
	print("SMOKE after_pen_touch=", canvas.stroke_count())
	# ...an untilted finger drag orbits instead of drawing...
	var rig = main.get("rig")
	var yaw_before: float = rig.yaw
	await _touch_stroke(0, center, Vector2(120, 0), Vector2.ZERO, 0.0)
	print("SMOKE finger_orbited=", rig.yaw != yaw_before, " strokes=", canvas.stroke_count())
	# ...and a quick two-finger tap undoes.
	_touch(0, center, true)
	_touch(1, center + Vector2(80, 0), true)
	_touch(0, center, false)
	_touch(1, center + Vector2(80, 0), false)
	await get_tree().process_frame
	print("SMOKE after_two_finger_tap=", canvas.stroke_count())
	_key(KEY_Z, true, true)
	print("SMOKE after_redo_marker=", canvas.stroke_count())

	# Dots brush, then a custom brush loaded from a user folder.
	canvas.set_brush("com.airdraw.dots")
	await _stroke(center + Vector2(-350, 200), func(t: float) -> Vector2: return Vector2(t * 700.0, sin(t * TAU) * 40.0))
	var dir := "user://test_brushes"
	DirAccess.make_dir_recursive_absolute(dir)
	var f := FileAccess.open(dir + "/glow.brush.toml", FileAccess.WRITE)
	f.store_string(GLOW_BRUSH)
	f.close()
	var loaded: int = canvas.load_brush_dir(ProjectSettings.globalize_path(dir))
	print("SMOKE custom_loaded=", loaded, " set=", canvas.set_brush("test.glow"))
	await _stroke(center + Vector2(-250, 330), func(t: float) -> Vector2: return Vector2(t * 500.0, -sin(t * PI) * 60.0))
	print("SMOKE final_strokes=", canvas.stroke_count(), " brushes=", canvas.brush_ids())

	# Document: re-brush (undoable), clear (undoable), save → new → open.
	canvas.set_brush("com.airdraw.ribbon")
	print("SMOKE rebrushed=", canvas.rebrush_last())
	canvas.undo()
	canvas.redo()
	canvas.clear()
	print("SMOKE after_clear=", canvas.stroke_count())
	canvas.undo()
	print("SMOKE after_clear_undo=", canvas.stroke_count())
	await main.update_thumbnail()
	var path := ProjectSettings.globalize_path("user://test_documents/smoke.airdraw")
	print("SMOKE save_error='", canvas.save_document(path), "' dirty=", canvas.is_dirty())
	var thumb: PackedByteArray = canvas.thumbnail_of(path)
	var thumb_img := Image.new()
	print("SMOKE thumbnail_bytes=", thumb.size(), " png_ok=", thumb_img.load_png_from_buffer(thumb) == OK, " size=", thumb_img.get_size())
	thumb_img.save_png("user://test_documents/smoke_thumbnail.png")
	canvas.new_document()
	print("SMOKE after_new=", canvas.stroke_count())
	print("SMOKE load_error='", canvas.load_document(path), "' strokes=", canvas.stroke_count(), " dirty=", canvas.is_dirty())
	# Undo history is saved with the sketch: clear, save, reopen, then undo brings the
	# strokes back (from the file) and redo clears them again.
	var before_clear: int = canvas.stroke_count()
	canvas.clear()
	var history_path := ProjectSettings.globalize_path("user://test_documents/history.airdraw")
	canvas.save_document(history_path)
	canvas.new_document()
	canvas.load_document(history_path)
	var reopened_empty: bool = canvas.stroke_count() == 0 and canvas.can_undo() and not canvas.is_dirty()
	canvas.undo()
	var restored: int = canvas.stroke_count()
	canvas.redo()
	var cleared_again: bool = canvas.stroke_count() == 0
	canvas.undo()
	print("SMOKE history reopened_empty=", reopened_empty, " undo_restores=", restored == before_clear, " redo_clears=", cleared_again,
		" meshes=", canvas.get_children().filter(func(c): return c is MeshInstance3D and c.visible).size() >= before_clear)
	canvas.load_document(path)
	# Save a Copy Without History: the copy has the strokes but no undo; the open sketch
	# keeps its history and isn't marked edited. (The menu item opens the system picker /
	# native dialog; that part is checked by hand.)
	canvas.clear()
	canvas.undo()
	var dirty_before: bool = canvas.is_dirty()
	var copy_path := ProjectSettings.globalize_path("user://test_documents/copy.airdraw")
	var copy_err: String = canvas.save_copy_without_history(copy_path)
	var kept_history: bool = canvas.can_undo() and canvas.can_redo() and canvas.is_dirty() == dirty_before
	var strokes_now: int = canvas.stroke_count()
	canvas.load_document(copy_path)
	print("SMOKE save_copy error='", copy_err, "' strokes=", canvas.stroke_count() == strokes_now, " no_history=", not canvas.can_undo() and not canvas.can_redo(),
		" original_kept_history=", kept_history, " menu_item=", main.get("toolbar").find_button("Save a copy without history") != null)
	canvas.load_document(path)
	# Toolbar by touch: tap the Glow brush and Undo buttons.
	var bar = main.get("toolbar")
	print("SMOKE glow_button=", bar.find_button("test.glow") != null)
	await _settle()
	await _tap(bar.find_button("Brushes"))
	await _settle()
	print("SMOKE sheet_opened=", bar.sheet_open)
	await _tap(bar.find_button("com.airdraw.dots"))
	print("SMOKE tapped_dots=", canvas.current_brush())
	await _tap(bar.find_button("test.glow"))
	print("SMOKE tapped_glow=", canvas.current_brush())
	await _stroke(center + Vector2(-200, 100), func(t: float) -> Vector2: return Vector2(t * 400.0, 0.0))
	print("SMOKE sheet_closed_by_drawing=", not bar.sheet_open, " strip_has_glow=", bar.recent_brushes.has("test.glow"))
	var before: int = canvas.stroke_count()
	await _tap(bar.find_button("Undo"))
	print("SMOKE tapped_undo=", before, "->", canvas.stroke_count())
	# Export through the dialog: GLB with curves, then OBJ with Z up.
	await _tap(bar.find_button("Export"))
	var dialog = main.get("export_dialog")
	print("SMOKE dialog_visible=", dialog.visible, " options=", dialog.current_options().keys())
	dialog._controls["curves"].button_pressed = true
	await get_tree().process_frame
	dialog.stem = "smoke"
	dialog._export()
	print("SMOKE export_glb=", main.get("status").replace("\n", " | "))
	dialog.open(canvas, bar.popup_theme())
	dialog._format_picker.select(2)
	dialog._build_options()
	dialog._controls["z_up"].button_pressed = true
	dialog._export()
	print("SMOKE export_obj=", main.get("status").replace("\n", " | "))
	var bad := FileAccess.open("user://test_documents/bad.airdraw", FileAccess.WRITE)
	bad.store_string("not a sketch")
	bad.close()
	print("SMOKE bad_load_error='", canvas.load_document(ProjectSettings.globalize_path("user://test_documents/bad.airdraw")), "' strokes_kept=", canvas.stroke_count())
	# Exporter plugin: the sample C++ plugin, built into user://test_plugins by the test runner.
	var plugin_msgs: PackedStringArray = canvas.load_exporter_plugins(ProjectSettings.globalize_path("user://test_plugins"))
	print("SMOKE plugin_load=", plugin_msgs)
	var formats := []
	for fmt in canvas.exporters():
		formats.append(fmt.extension)
	print("SMOKE formats=", formats)
	dialog.open(canvas, bar.popup_theme())
	dialog._format_picker.select(formats.find("sem"))
	dialog._build_options()
	dialog._controls["engine_units_per_meter"].value = 100.0
	dialog._controls["also_glb"].button_pressed = true
	dialog._controls["curves"].button_pressed = true
	dialog._export()
	print("SMOKE export_plugin=", main.get("status").replace("\n", " | "))

	# Every registered format through the app's export path, for external verification.
	for fmt in canvas.exporters():
		var r: Dictionary = canvas.export_document(fmt.id, ProjectSettings.globalize_path("user://exports/all." + fmt.extension), {})
		print("SMOKE format ", fmt.extension, " error='", r.error, "' files=", r.files.size(), " warnings=", r.warnings)

	# Brush editor: tweak the current brush, save as new, draw with it, share, import, delete.
	await _tap(bar.find_button("Edit current brush"))
	var editor = main.get("brush_editor")
	await get_tree().process_frame
	print("SMOKE editor_visible=", editor.visible, " preview=", editor._preview.mesh != null, " error='", editor._error.text, "'")
	var count_before: int = canvas.brush_ids().size()
	editor.def.name = "Thick Glow"
	editor.def.dynamics[0].params.base = 0.06
	editor.def.dynamics[0].params.pressure_curve = PackedVector2Array([Vector2(0, 1), Vector2(1, 0.2)])
	editor.def.filters.append({"type": "spline", "params": {"steps": 4.0}})
	editor.refresh_preview()
	print("SMOKE edited_preview_error='", editor._error.text, "'")
	editor.save_as_new()
	var new_id: String = canvas.current_brush()
	print("SMOKE saved_new=", new_id, " brushes=", count_before, "->", canvas.brush_ids().size())
	# Saving from the editor keeps filters and dynamics (they used to be dropped on the way back
	# from GDScript, so the edited width was lost).
	var saved: Dictionary = canvas.brush_definition(new_id)
	print("SMOKE saved_keeps_dynamics=", saved.dynamics.size() > 0 and is_equal_approx(saved.dynamics[0].params.base, 0.06),
		" filters=", saved.filters.size())
	var thick := func(id: String) -> float: return canvas.brush_preview(canvas.brush_definition(id)).mesh.get_aabb().size.z
	print("SMOKE preview_marker_thicker_than_ink=", thick.call("com.airdraw.marker") > thick.call("com.airdraw.ink") * 2.0)
	var strokes_before: int = canvas.stroke_count()
	await _stroke(center + Vector2(-300, 60), func(t: float) -> Vector2: return Vector2(t * 250.0, sin(t * TAU) * 30.0))
	print("SMOKE drew_with_new=", canvas.stroke_count() - strokes_before)
	editor.def.geometry.params["bogus"] = 1
	editor.refresh_preview()
	print("SMOKE bad_param_error='", editor._error.text, "'")
	editor.def.geometry.params.erase("bogus")
	editor.refresh_preview()
	editor.share()
	var shared := AppPaths.exports().path_join("%s.brush.toml" % new_id.validate_filename())
	print("SMOKE shared_exists=", FileAccess.file_exists(shared))
	var import_file := AppPaths.import_dir().path_join("friend.brush.toml")
	var out := FileAccess.open(import_file, FileAccess.WRITE)
	out.store_string(FileAccess.get_file_as_string(shared).replace(new_id, "friend.imported"))
	out.close()
	editor.delete()
	print("SMOKE after_delete=", canvas.brush_ids().has(new_id), " editor_visible=", editor.visible)
	await _settle()
	await _tap(bar.find_button("Brushes"))
	await _settle()
	await _tap(bar.find_button("Import"))
	print("SMOKE imported=", canvas.brush_ids().has("friend.imported"), " status=", main.get("status"))
	DirAccess.remove_absolute(import_file)
	canvas.delete_brush("friend.imported")
	bar.close_sheet()

	# Leave the brush editor open for the screenshot so its layout can be reviewed.
	bar.close_sheet()
	await _tap(bar.find_button("Edit current brush"))
	await get_tree().process_frame
	await get_tree().process_frame
	get_viewport().get_texture().get_image().save_png("user://smoke.png")
	print("SMOKE screenshot=", ProjectSettings.globalize_path("user://smoke.png"))

	# A clean sketch for visual checks of 2D/AR exports: spiral (Ink) + loop (Ribbon) on the front plane.
	main.get("brush_editor").close()
	await main.new_sketch()
	_key(KEY_1)
	canvas.set_brush("com.airdraw.ink")
	await _stroke(center + Vector2(-150, 0), func(t: float) -> Vector2: return Vector2(cos(t * 12.0), sin(t * 12.0)) * (20.0 + t * 160.0))
	canvas.set_brush("com.airdraw.ribbon")
	await _stroke(center + Vector2(200, 0), func(t: float) -> Vector2: return Vector2(cos(t * TAU) * 120.0, sin(t * TAU) * 160.0))
	for ext in ["svg", "usdz"]:
		var id: String = "com.airdraw." + ext
		var r: Dictionary = canvas.export_document(id, ProjectSettings.globalize_path("user://exports/clean." + ext), {})
		print("SMOKE clean_", ext, " error='", r.error, "'")
	# 3D guide (Feather-style): from the front, a horizontal guide stroke becomes a sheet
	# running away from the viewer; from the top, a stroke drawn across it lands at real depth.
	var shell = main.get("toolbar")
	var guide_before: int = canvas.stroke_count()
	await _tap(shell.find_button("Guide"))
	var mode_on: bool = main.get("session").guide_drawing
	await _stroke(center, func(t: float) -> Vector2: return Vector2(-250.0 + 500.0 * t, 0.0))
	print("SMOKE guide mode_on=", mode_on, " drawn=", canvas.has_drawn_guide(), " mode_after=", main.get("session").guide_drawing,
		" tool_after=", main.get("tool"), " draw_button=", shell.find_button("Draw").button_pressed, " strokes_unchanged=", canvas.stroke_count() == guide_before)
	rig.set_view("top")
	await get_tree().create_timer(0.5).timeout
	await _stroke(center, func(t: float) -> Vector2: return Vector2(0.0, -150.0 + 300.0 * t))
	var depth: float = canvas.strokes_aabb().size.z
	print("SMOKE guide stroke_added=", canvas.stroke_count() == guide_before + 1, " drawn_on_guide_depth_ok=", depth > 0.5)
	await _shot("user://guide_top.png")
	rig.set_view("3d")
	await get_tree().create_timer(0.5).timeout
	await _shot("user://guide_3d.png")
	_key(KEY_Z, true)
	_key(KEY_Z, true)
	var undone: bool = not canvas.has_drawn_guide()
	_key(KEY_Z, true, true)
	print("SMOKE guide undo_removes=", undone, " redo_restores=", canvas.has_drawn_guide())
	# The drawn guide is saved with the sketch (format 3) and shown again on open.
	var guide_path := ProjectSettings.globalize_path("user://test_documents/guide.airdraw")
	var guide_line: PackedVector3Array = canvas.guide_start_line()
	canvas.save_document(guide_path)
	canvas.new_document()
	var cleared: bool = not canvas.has_drawn_guide() and not main.get("guides")._drawn.visible
	canvas.load_document(guide_path)
	print("SMOKE guide new_clears=", cleared, " reloaded=", canvas.has_drawn_guide(), " shown=", main.get("guides")._drawn.visible,
		" same_shape=", canvas.guide_start_line() == guide_line, " not_dirty=", not canvas.is_dirty())
	# Opacity: the Guide bar slider sets it on the guide's material. It's a device setting:
	# it never dirties the sketch and survives switching sketches.
	main.set_tool("guide")
	var guides = main.get("guides")
	var opacity_slider: HSlider = shell.find_child("Guide opacity", true, false)
	opacity_slider.value = 0.6
	rig.set_view("3d")
	await get_tree().create_timer(0.6).timeout
	await _shot("user://guide_opacity.png")
	rig.set_view("front")
	var drawn_fill: Color = guides._drawn.material_override.get_shader_parameter("fill_color")
	var settings := ConfigFile.new()
	settings.load(GuideController.SETTINGS)
	var stored: float = settings.get_value("guide", "opacity", -1.0)
	var dirty_after_opacity: bool = canvas.is_dirty()
	canvas.new_document()
	print("SMOKE guide_opacity fill=", is_equal_approx(drawn_fill.a, 0.6), " not_dirty=", not dirty_after_opacity,
		" device_setting=", is_equal_approx(stored, 0.6), " kept_on_new=", is_equal_approx(guides.opacity(), 0.6),
		" plane_fill=", is_equal_approx(guides._guide.material_override.get_shader_parameter("fill_color").a, 0.6))
	guides.set_opacity(GuideController.DEFAULT_OPACITY)
	guides._save_opacity()
	# Viewing never rewrites a file: open it, orbit, change opacity, leave: same bytes, same
	# modification time. An edit does save.
	var file_before := FileAccess.get_file_as_bytes(guide_path)
	var mtime := FileAccess.get_modified_time(guide_path)
	await main.get("documents").open_path(guide_path, "")
	rig.orbit(Vector2(0.4, 0.2))
	guides.set_opacity(0.3)
	guides.set_opacity(GuideController.DEFAULT_OPACITY)
	await main.get("documents").save_current()
	var unchanged := FileAccess.get_file_as_bytes(guide_path) == file_before and FileAccess.get_modified_time(guide_path) == mtime
	canvas.remove_guide()
	await main.get("documents").save_current()
	var edited_saved := FileAccess.get_file_as_bytes(guide_path) != file_before
	print("SMOKE view_only unchanged=", unchanged, " edit_saves=", edited_saved)
	# The preview is rendered from a fixed angle: two very different cameras, same PNG.
	var renderer: ThumbnailRenderer = main.get("thumbnails")
	rig.set_view("top")
	await get_tree().create_timer(0.3).timeout
	var png_top: PackedByteArray = await renderer.render(canvas)
	rig.set_view("side")
	await get_tree().create_timer(0.3).timeout
	var png_side: PackedByteArray = await renderer.render(canvas)
	rig.set_view("front")
	print("SMOKE thumbnail_fixed_angle same=", png_top == png_side, " not_empty=", png_top.size() > 0,
		" content_id=", canvas.content_id().length() == 32)
	canvas.load_document(guide_path)
	main.set_tool("draw")
	shell.toast(main.GUIDE_READY)
	main.get("guides").set_preset(0)
	print("SMOKE guide preset_replaces=", not canvas.has_drawn_guide(), " stale_toast_hidden=", not shell._toast.visible)
	rig.set_view("front")
	await get_tree().create_timer(0.5).timeout

	# Looks: pick Clay from the ⋯ menu by touch; every stroke switches shader, the lighting
	# follows, and the Look is saved with the sketch. A new sketch starts Shaded.
	await _tap(shell.find_button("More"))
	await _settle()
	await _tap(shell.find_button("Look Clay"))
	await _settle()
	var stroke_shaders := []
	for c in canvas.get_children():
		if c is MeshInstance3D and c.material_override is ShaderMaterial:
			stroke_shaders.append("diffuse_lambert_wrap" in c.material_override.shader.code)
	var env: Environment = main.get("environment").environment
	print("SMOKE look picked=", canvas.look(), " strokes_clay=", stroke_shaders.size() > 0 and stroke_shaders.all(func(x): return x),
		" lighting_follows=", env.background_color.is_equal_approx(canvas.look_environment().background), " shadows=", main.get("sun").shadow_enabled)
	await _shot("user://look_clay.png")
	var look_path := ProjectSettings.globalize_path("user://test_documents/look.airdraw")
	canvas.save_document(look_path)
	canvas.new_document()
	var fresh_look: String = canvas.look()
	canvas.load_document(look_path)
	print("SMOKE look new_sketch=", fresh_look, " reloaded=", canvas.look(), " env_reloaded=",
		env.background_color.is_equal_approx(canvas.look_environment().background))
	# Toon and Ink add an outline pass to every stroke; Shaded has none.
	var outlined := func() -> Array:
		var found := []
		for c in canvas.get_children():
			if c is MeshInstance3D and c.material_override is ShaderMaterial:
				var next: Material = c.material_override.next_pass
				found.append(next != null and "cull_" in next.shader.code and "outline_width" in next.shader.code)
		return found
	for look in ["Toon", "Ink"]:
		await _tap(shell.find_button("More"))
		await _settle()
		await _tap(shell.find_button("Look " + look))
		await _settle()
		var o: Array = outlined.call()
		print("SMOKE look ", look.to_lower(), "=", canvas.look() == "com.airdraw." + look.to_lower(), " outlined=", o.size() > 0 and o.all(func(x): return x))
		await _shot("user://look_%s.png" % look.to_lower())
	await _tap(shell.find_button("More"))
	await _settle()
	await _tap(shell.find_button("Look Shaded"))
	await _settle()
	print("SMOKE look back_to=", canvas.look(), " outlines_gone=", not (outlined.call() as Array).any(func(x): return x))

	# Editor layouts: landscape, portrait (bottom sheet), left-handed.
	await _shot("user://layout_landscape.png")
	var win := get_window()
	var original_size := win.size
	win.size = Vector2i(820, 1180)
	await _shot("user://layout_portrait.png")
	await _tap(shell.find_button("Brushes"))
	await _shot("user://layout_portrait_sheet.png")
	print("SMOKE portrait=", shell.portrait, " sheet_open=", shell.sheet_open)
	shell.close_sheet()
	win.size = original_size
	shell.left_handed = true
	shell._layout()
	await _shot("user://layout_left_handed.png")
	print("SMOKE left_handed rail_x=", shell._rail.position.x, " cube_x=", shell._cube.position.x)
	shell.left_handed = false
	shell._layout()
	print("SMOKE ui_rects=", shell.ui_rects().size())
	# Desktop shortcuts: G arms the Guide tool, B goes back; Tab hides the interface.
	_key(KEY_G)
	var g_tool: String = main.get("tool")
	_key(KEY_B)
	print("SMOKE keys g=", g_tool, " b=", main.get("tool"), " guide_tooltip='", shell.find_button("Guide").tooltip_text, "'")
	_key(KEY_TAB)
	await get_tree().process_frame
	await _shot("user://layout_hidden.png")
	print("SMOKE hidden=", shell.ui_hidden, " rects=", shell.ui_rects().size())
	_key(KEY_TAB)
	# Four-finger tap (touch path) toggles it too.
	for i in 4:
		_touch(10 + i, center + Vector2(i * 60, 0), true)
	for i in 4:
		_touch(10 + i, center + Vector2(i * 60, 0), false)
	await get_tree().process_frame
	var four_hid: bool = shell.ui_hidden
	shell.toggle_ui()
	print("SMOKE four_finger_hides=", four_hid, " restored=", not shell.ui_hidden)
	# View cube: tapping its top face (seen from the front view, the top face is hidden; from
	# 3D it's above the center) switches views.
	rig.set_view("3d")
	await get_tree().create_timer(0.5).timeout
	var cube = shell._cube
	var top_face: String = cube._face_at(cube.size / 2 + Vector2(0, -cube.size.y * 0.25))
	print("SMOKE cube_face_above_center=", top_face)
	# From straight on, tapping right of the cube turns to the right face (touch, as on iPad).
	rig.set_view("front")
	await get_tree().create_timer(0.5).timeout
	await _shot("user://cube_front.png")
	var beside: Vector2 = cube.get_global_rect().get_center() + Vector2(cube._radius() * 1.3, 0)
	_touch(6, beside, true)
	_touch(6, beside, false)
	await get_tree().create_timer(0.5).timeout
	print("SMOKE cube_tap_right_turns=", is_equal_approx(rig.yaw, PI / 2), " now_facing=", cube._face_at(cube.size / 2))
	rig.set_view("3d")
	await get_tree().create_timer(0.5).timeout

	# The ⋯ menu by touch: it opens, a tap elsewhere closes it without drawing, its items work,
	# and the rest of the interface keeps working afterwards (it used to block all input on iPad).
	var strokes_before_menu: int = canvas.stroke_count()
	await _tap(shell.find_button("More"))
	var menu_opened: bool = shell._menu.visible
	await _shot("user://menu_open.png")
	_touch(7, center + Vector2(0, 80), true)
	_touch(7, center + Vector2(0, 80), false)
	await _settle()
	print("SMOKE menu opened=", menu_opened, " outside_tap_closes=", not shell._menu.visible,
		" no_stroke=", canvas.stroke_count() == strokes_before_menu)
	await _tap(shell.find_button("More"))
	await _settle()
	await _tap(shell.find_button("Left-handed mode"))
	await _settle()
	var lh: bool = shell.left_handed
	await _tap(shell.find_button("More"))
	await _settle()
	await _tap(shell.find_button("Left-handed mode"))
	await _settle()
	await _tap(shell.find_button("Guide"))
	print("SMOKE menu item_toggles=", lh, " item_closes=", not shell._menu.visible, " back=", not shell.left_handed,
		" ui_works_after=", main.get("tool") == "guide")
	# Look rows are rebuilt each time the menu opens: still one per Look, the current one ticked.
	await _tap(shell.find_button("More"))
	await _settle()
	var look_rows: int = shell._look_box.get_child_count()
	var ticked: int = shell._look_box.get_children().filter(func(b): return b.text.begins_with("✓")).size()
	shell.close_menu()
	print("SMOKE menu look_rows=", look_rows == canvas.looks().size(), " one_ticked=", ticked == 1,
		" closes=", not shell._menu.visible)
	await _tap(shell.find_button("Draw"))

	# Erase (§3i): picking projects like Godot's camera; a drag hides what it touches and
	# removes it on lift as one undo step; E and Pencil double-tap switch tools.
	var session = main.get("session")
	var worst := 0.0
	for w in [Vector3.ZERO, Vector3(0.4, 0.3, -0.2), Vector3(-0.7, 0.1, 0.5)]:
		worst = maxf(worst, session.pick_project(w).distance_to(rig.camera.unproject_position(w)))
	print("SMOKE erase projection_matches_godot=", worst < 0.5, " pickable_in_step=", canvas.pickable_count() == canvas.stroke_count())
	var line_y := center.y - 60.0
	await _stroke(Vector2(center.x - 150, line_y), func(t: float) -> Vector2: return Vector2(t * 300.0, 0.0))
	var with_line: int = canvas.stroke_count()
	_key(KEY_E)
	var tool_is_erase: bool = main.get("tool") == "erase" and shell.find_button("Erase").button_pressed
	_mouse_button(Vector2(center.x - 160, line_y), true)
	for i in range(1, 41):
		var ev := InputEventMouseMotion.new()
		ev.position = Vector2(center.x - 160 + i * 8.0, line_y + sin(i * 0.5) * 4.0)
		ev.button_mask = MOUSE_BUTTON_MASK_LEFT
		Input.parse_input_event(ev)
	Input.flush_buffered_events()
	var mid_drag: bool = canvas.is_erasing() and canvas.stroke_count() == with_line and shell._eraser_ring.visible
	await _shot("user://erase_drag.png")
	_mouse_button(Vector2(center.x + 170, line_y), false)
	await get_tree().process_frame
	var after_erase: int = canvas.stroke_count()
	_key(KEY_Z, true)
	var erase_undone: int = canvas.stroke_count()
	_key(KEY_Z, true, true)
	print("SMOKE erase tool=", tool_is_erase, " hides_until_lift=", mid_drag, " erased=", after_erase < with_line,
		" no_ink=", after_erase <= with_line - 1, " ring_hidden_after=", not shell._eraser_ring.visible,
		" one_undo_step=", erase_undone == with_line, " redo=", canvas.stroke_count() == after_erase,
		" pickable_in_step=", canvas.pickable_count() == canvas.stroke_count())
	# Leave the sketch as it was for the checks below (the erase may have emptied it).
	_key(KEY_Z, true)
	# Partial: a drag across a line cuts it into pieces, shown live, kept as one undo step.
	await _tap(shell.find_button("Erase partial"))
	var partial_set: bool = session.erase_partial and shell.find_button("Erase partial").button_pressed
	var row := center.y + 40.0
	main.set_tool("draw")
	await _stroke(Vector2(center.x - 150, row), func(t: float) -> Vector2: return Vector2(t * 300.0, 0.0))
	var before_cut: int = canvas.stroke_count()
	main.set_tool("erase")
	var shown := func() -> int:
		return canvas.get_children().filter(func(c): return c is MeshInstance3D and c.visible and c.mesh != null).size()
	var shown_before: int = shown.call()
	await _erase_drag(Vector2(center.x, row - 80), Vector2(center.x + 4, row + 80), func() -> void:
		# The cut line is hidden and its two pieces shown (other strokes the drag cuts add more).
		print("SMOKE erase partial_live=", canvas.is_erasing() and canvas.stroke_count() == before_cut,
			" pieces_shown=", shown.call() > shown_before))
	var after_cut: int = canvas.stroke_count()
	_key(KEY_Z, true)
	var cut_undone: int = canvas.stroke_count()
	_key(KEY_Z, true, true)
	print("SMOKE erase partial set=", partial_set, " split=", after_cut > before_cut, " one_undo_step=", cut_undone == before_cut,
		" redo=", canvas.stroke_count() == after_cut, " pickable_in_step=", canvas.pickable_count() == canvas.stroke_count())
	_key(KEY_Z, true)
	# Holding Shift when the drag starts erases whole strokes instead.
	var shift := InputEventKey.new()
	shift.keycode = KEY_SHIFT
	shift.pressed = true
	Input.parse_input_event(shift)
	await _erase_drag(Vector2(center.x, row - 80), Vector2(center.x + 4, row + 80), func() -> void: pass)
	var shift_up := shift.duplicate()
	shift_up.pressed = false
	Input.parse_input_event(shift_up)
	Input.flush_buffered_events()
	print("SMOKE erase shift_whole=", canvas.stroke_count() < before_cut)
	_key(KEY_Z, true)
	await _tap(shell.find_button("Erase whole"))
	print("SMOKE erase back_to_whole=", not session.erase_partial, " sketch_restored=", canvas.stroke_count() == before_cut)
	# Select (§3i): V, tap a stroke (accent outline), Esc, lasso, duplicate as one undo step,
	# select all, and the stroke menu by right-click and long-press.
	var sel_row := center.y + 120.0
	main.set_tool("draw")
	await _stroke(Vector2(center.x - 150, sel_row), func(t: float) -> Vector2: return Vector2(t * 300.0, 0.0))
	var sel_strokes: int = canvas.stroke_count()
	_key(KEY_V)
	var sel_v_selects: bool = main.get("tool") == "select" and shell.find_button("Select").button_pressed
	await _click(Vector2(center.x, sel_row), MOUSE_BUTTON_LEFT)
	var sel_outlined := func() -> int:
		return canvas.get_children().filter(func(c): return c is MeshInstance3D and c.visible and c.material_overlay != null).size()
	var sel_tapped: int = canvas.selection_count()
	var sel_outline_matches: bool = sel_outlined.call() == sel_tapped
	var sel_bar_says: bool = shell.find_button("Delete") != null and not shell.find_button("Delete").disabled
	await _shot("user://select_tap.png")
	_key(KEY_ESCAPE)
	var sel_esc_clears: bool = canvas.selection_count() == 0 and sel_outlined.call() == 0 and shell.find_button("Delete").disabled
	print("SMOKE select v=", sel_v_selects, " tap=", sel_tapped == 1, " outlined=", sel_outline_matches, " bar=", sel_bar_says, " esc_clears=", sel_esc_clears)
	var sel_box := [Vector2(center.x - 170, sel_row - 25), Vector2(center.x + 170, sel_row - 25), Vector2(center.x + 170, sel_row + 25), Vector2(center.x - 170, sel_row + 25), Vector2(center.x - 170, sel_row - 25)]
	await _erase_drag(sel_box[0], sel_box[0], func() -> void: pass)  # a tap on nothing clears
	_mouse_button(sel_box[0], true)
	for i in range(1, 5):
		for k in 10:
			var sel_ev := InputEventMouseMotion.new()
			sel_ev.position = sel_box[i - 1].lerp(sel_box[i], (k + 1) / 10.0)
			sel_ev.button_mask = MOUSE_BUTTON_MASK_LEFT
			Input.parse_input_event(sel_ev)
	Input.flush_buffered_events()
	var sel_lasso_drawn: bool = shell._lasso.points.size() > 3
	_mouse_button(sel_box[4], false)
	await get_tree().process_frame
	var sel_lassoed: int = canvas.selection_count()
	_key(KEY_D, true)
	var sel_duplicated: bool = canvas.stroke_count() == sel_strokes + sel_lassoed and canvas.selection_count() == sel_lassoed
	_key(KEY_Z, true)
	var sel_dup_undone: bool = canvas.stroke_count() == sel_strokes
	_key(KEY_A, true)
	var sel_all: bool = canvas.selection_count() == canvas.stroke_count()
	_key(KEY_ESCAPE)
	print("SMOKE select lasso=", sel_lassoed >= 1, " lasso_shown=", sel_lasso_drawn, " lasso_cleared=", shell._lasso.points.is_empty(),
		" duplicate=", sel_duplicated, " one_undo_step=", sel_dup_undone, " all=", sel_all)
	main.set_tool("draw")
	await _click(Vector2(center.x, sel_row), MOUSE_BUTTON_RIGHT)
	await _settle()
	var sel_menu_by_click: bool = shell._stroke_menu.visible and canvas.selection_count() == 1 and main.get("tool") == "select"
	await _tap(shell.find_button("Menu Delete"))
	var sel_menu_deleted: bool = canvas.stroke_count() == sel_strokes - 1 and not shell._stroke_menu.visible
	_key(KEY_Z, true)
	_touch(0, Vector2(center.x, sel_row), true)
	await get_tree().create_timer(0.7).timeout
	var sel_menu_by_hold: bool = shell._stroke_menu.visible and canvas.selection_count() == 1
	_touch(0, Vector2(center.x, sel_row), false)
	shell.close_menu()
	_key(KEY_BACKSPACE)
	var sel_backspace: bool = canvas.stroke_count() == sel_strokes - 1
	_key(KEY_Z, true)
	# Mouse events right after a touch are taken as the OS's copies of it and dropped.
	await get_tree().create_timer(0.3).timeout
	await _click(Vector2(center.x, sel_row), MOUSE_BUTTON_LEFT)
	canvas.set_brush("com.airdraw.dots")
	var sel_rebrushed: bool = canvas.rebrush_selected()
	if sel_rebrushed:
		_key(KEY_Z, true)
	_key(KEY_ESCAPE)
	print("SMOKE select menu_right_click=", sel_menu_by_click, " menu_delete=", sel_menu_deleted, " menu_long_press=", sel_menu_by_hold,
		" backspace=", sel_backspace, " rebrush=", sel_rebrushed, " restored=", canvas.stroke_count() == sel_strokes,
		" pickable_in_step=", canvas.pickable_count() == canvas.stroke_count())
	# Transform gizmo (§3i step 5): drag the pad to move, the ring to turn; each drag bakes
	# into the strokes as one undo step; mid-drag nothing is recorded.
	main.set_tool("select")
	await get_tree().create_timer(0.3).timeout
	await _click(Vector2(center.x, sel_row), MOUSE_BUTTON_LEFT)
	var gizmo = shell._gizmo
	await _settle()
	var gz_shown: bool = gizmo.visible and canvas.selection_count() == 1
	# Race to idle (§10): an untouched canvas with a selection and its gizmo draws nothing.
	var idle_selection_frames: int = await _idle_frames()
	var on_ui := func(p: Vector2) -> bool: return shell.ui_rects().any(func(r: Rect2) -> bool: return r.has_point(p))
	# The handles are UI; inside the ring and between handles strokes stay reachable.
	var gz_in_rects: bool = on_ui.call(gizmo._at) and on_ui.call(gizmo._at + Vector2(gizmo.RING * gizmo.ui_scale, 0)) \
		and not on_ui.call(gizmo._at + Vector2(0, gizmo.RING * gizmo.ui_scale * 0.6))
	var c0: Vector3 = canvas.selection_center()
	var at0: Vector2 = gizmo._at
	var undo_before: int = canvas.stroke_count()
	_mouse_button(at0, true)
	for i in range(1, 21):
		var ev := InputEventMouseMotion.new()
		ev.position = at0 + Vector2(80, 0) * (i / 20.0)
		ev.button_mask = MOUSE_BUTTON_MASK_LEFT
		Input.parse_input_event(ev)
	Input.flush_buffered_events()
	var gz_mid_unbaked: bool = canvas.selection_center().is_equal_approx(c0) and not canvas.is_erasing() and shell._lasso.points.is_empty()
	_mouse_button(at0 + Vector2(80, 0), false)
	await _settle()
	var c1: Vector3 = canvas.selection_center()
	var gz_moved: bool = rig.camera.unproject_position(c1).distance_to(at0 + Vector2(80, 0)) < 2.0 and canvas.selection_count() == 1 \
		and canvas.stroke_count() == undo_before
	# A quarter turn clockwise on the ring: the horizontal line stands up.
	var at1: Vector2 = gizmo._at
	var r: float = gizmo.RING * gizmo.ui_scale
	_mouse_button(at1 + Vector2(r, 0), true)
	for i in range(1, 21):
		var ev := InputEventMouseMotion.new()
		ev.position = at1 + Vector2.from_angle(PI / 2 * i / 20.0) * r
		ev.button_mask = MOUSE_BUTTON_MASK_LEFT
		Input.parse_input_event(ev)
	Input.flush_buffered_events()
	_mouse_button(at1 + Vector2(0, r), false)
	await _settle()
	await _shot("user://gizmo_turned.png")
	var gz_turn_keeps_center: bool = canvas.selection_center().distance_to(c1) < 0.01
	# Depth bar up pushes away; the top arc (Shift: 15° steps) turns about world up.
	var eye: Vector3 = rig.camera.global_position
	var far_before: float = eye.distance_to(canvas.selection_center())
	var depth_bar: Rect2 = gizmo._bar_rect()
	await _gizmo_drag(depth_bar.get_center(), depth_bar.get_center() + Vector2(0, -40))
	var gz_pushed: bool = eye.distance_to(canvas.selection_center()) > far_before + 0.05
	var box_before: AABB = canvas.strokes_aabb()
	var yaw_at: Vector2 = gizmo._at + Vector2.from_angle(deg_to_rad(-90.0)) * gizmo.ARC * gizmo.ui_scale
	await _gizmo_drag(yaw_at, yaw_at + Vector2(157, 0))
	var gz_yawed: bool = not canvas.strokes_aabb().is_equal_approx(box_before) and canvas.selection_count() == 1
	_key(KEY_Z, true)
	_key(KEY_Z, true)
	_key(KEY_ESCAPE)
	var gz_hidden_without_selection: bool = not gizmo.visible
	await _click(at1 + Vector2(0, -100), MOUSE_BUTTON_LEFT)
	var gz_turned: bool = canvas.selection_count() == 1
	_key(KEY_ESCAPE)
	_key(KEY_Z, true)
	_key(KEY_Z, true)
	await _click(Vector2(center.x, sel_row), MOUSE_BUTTON_LEFT)
	var gz_undone: bool = canvas.selection_count() == 1 and canvas.selection_center().is_equal_approx(c0)
	# The same with a finger or Pencil (iPad: touches don't become mouse events), then undo.
	var touch_at: Vector2 = gizmo._at
	_touch(0, touch_at, true)
	for i in range(1, 21):
		var drag := InputEventScreenDrag.new()
		drag.index = 0
		drag.position = touch_at + Vector2(0, 60) * (i / 20.0)
		Input.parse_input_event(drag)
	Input.flush_buffered_events()
	Input.flush_buffered_events()
	_touch(0, touch_at + Vector2(0, 60), false)
	await _settle()
	var gz_touch_moved: bool = rig.camera.unproject_position(canvas.selection_center()).distance_to(touch_at + Vector2(0, 60)) < 2.0
	_key(KEY_Z, true)
	await _settle()
	gz_moved = gz_moved and gz_touch_moved
	_key(KEY_ESCAPE)
	print("SMOKE gizmo shown=", gz_shown, " in_ui_rects=", gz_in_rects, " mid_drag_unbaked=", gz_mid_unbaked, " moved_under_pen=", gz_moved,
		" turn_keeps_center=", gz_turn_keeps_center, " depth_pushes=", gz_pushed, " arc_turns=", gz_yawed, " turned_upright=", gz_turned, " hides=", gz_hidden_without_selection,
		" undo_restores=", gz_undone, " pickable_in_step=", canvas.pickable_count() == canvas.stroke_count())
	main.set_tool("draw")
	main.set_tool("erase")
	main.call("_pen_action", "doubletap")
	var back_to_draw: bool = main.get("tool") == "draw"
	main.call("_pen_action", "doubletap")
	print("SMOKE erase double_tap_toggles=", back_to_draw and main.get("tool") == "erase")
	await _tap(shell.find_button("Draw"))

	# Home screen: Gallery autosaves; both sketches are in Recents with thumbnails.
	await _tap(bar.find_button("Gallery"))
	for i in 3:
		await get_tree().process_frame
	var home = main.get("home")
	var names := []
	for e in main.get("app_files").recents():
		names.append(e.name)
	var idle_home_frames: int = await _idle_frames()
	print("SMOKE idle low_processor=", OS.low_processor_usage_mode, " home_draws_nothing=", idle_home_frames == 0,
		" selection_draws_nothing=", idle_selection_frames == 0, " frames=", [idle_home_frames, idle_selection_frames])
	var cards = home.cards()
	var with_thumbs: int = cards.filter(func(c): return c.get_node("Box/Tile/Thumb").texture != null).size()
	print("SMOKE home_visible=", home.visible, " recents=", names, " cards=", cards.size(), " with_thumbnails=", with_thumbs)
	get_viewport().get_texture().get_image().save_png("user://home.png")
	var landscape_columns: int = home._grid.columns
	var win_home := get_window()
	var home_size := win_home.size
	win_home.size = Vector2i(820, 1180)
	await _shot("user://home_portrait.png")
	var portrait_columns: int = home._grid.columns
	var row_fills: bool = absf(cards[0].size.x * portrait_columns + home._grid.get_theme_constant("h_separation") * (portrait_columns - 1) - home._grid.size.x) < 2.0
	win_home.size = home_size
	await _settle()
	print("SMOKE home_layout landscape_columns=", landscape_columns, " portrait_columns=", portrait_columns, " row_fills=", row_fills, " new_card_first=", home._grid.get_child(0).name == "NewSketch")

	# Recents: one file under two spellings of its path (an older build's absolute
	# /private/… path and the relative form) is one entry after tidying, and one card.
	var app_files = main.get("app_files")
	var dup := AppPaths.sketches().path_join("Dup.airdraw")
	DirAccess.copy_absolute(main.get("documents").resolve(app_files.recents()[0]), dup)
	app_files.touch_recent("/private" + dup, "")
	app_files.touch_recent(AppPaths.store(dup), "")
	app_files.touch_recent("/private" + dup, "")
	var spellings_before: int = app_files.recents().size()
	main.get("documents").tidy_recents()
	var dup_entries: int = app_files.recents().filter(func(e): return e.name == "Dup").size()
	print("SMOKE recents_dedupe stored_relative=", AppPaths.store("/private" + dup) == AppPaths.store(dup) and AppPaths.store(dup).begins_with(AppPaths.PREFIX),
		" tidied_to_one=", dup_entries == 1 and app_files.recents().size() == spellings_before - 1,
		" one_card=", main.get("documents").recent_items().size() == spellings_before - 1)
	app_files.remove_recent(AppPaths.store(dup))
	DirAccess.remove_absolute(dup)
	main.show_home()
	await _settle()
	cards = home.cards()
	await _tap(cards[1])
	await get_tree().process_frame
	print("SMOKE reopened=", main.get("doc_path").get_file(), " strokes=", canvas.stroke_count(), " home_visible=", home.visible)
	var svg_img := Image.new()
	svg_img.load_svg_from_string(FileAccess.get_file_as_string("user://exports/clean.svg"), 2.0)
	svg_img.save_png("user://exports/clean_svg_render.png")
	print("SMOKE clean_svg_render=", svg_img.get_size())
	get_tree().quit()


func _stroke(origin: Vector2, path: Callable) -> void:
	_mouse_button(origin + path.call(0.0), true)
	for i in range(1, 121):
		var t := i / 120.0
		var ev := InputEventMouseMotion.new()
		ev.position = origin + path.call(t)
		ev.button_mask = MOUSE_BUTTON_MASK_LEFT
		ev.pressure = 0.3 + 0.7 * sin(t * PI)
		Input.parse_input_event(ev)
		if i % 10 == 0:
			await get_tree().process_frame
	_mouse_button(origin + path.call(1.0), false)
	await get_tree().process_frame


## A mouse drag with the current tool from `a` to `b`; `mid` runs before the button is released.
func _erase_drag(a: Vector2, b: Vector2, mid: Callable) -> void:
	_mouse_button(a, true)
	for i in range(1, 31):
		var ev := InputEventMouseMotion.new()
		ev.position = a.lerp(b, i / 30.0)
		ev.button_mask = MOUSE_BUTTON_MASK_LEFT
		Input.parse_input_event(ev)
	Input.flush_buffered_events()
	mid.call()
	_mouse_button(b, false)
	await get_tree().process_frame


func _click(pos: Vector2, button: MouseButton) -> void:
	for pressed in [true, false]:
		var ev := InputEventMouseButton.new()
		ev.button_index = button
		ev.position = pos
		ev.pressed = pressed
		Input.parse_input_event(ev)
	Input.flush_buffered_events()
	await get_tree().process_frame


## A mouse drag on the gizmo from `a` to `b`.
func _gizmo_drag(a: Vector2, b: Vector2) -> void:
	_mouse_button(a, true)
	for i in range(1, 21):
		var ev := InputEventMouseMotion.new()
		ev.position = a.lerp(b, i / 20.0)
		ev.button_mask = MOUSE_BUTTON_MASK_LEFT
		Input.parse_input_event(ev)
	Input.flush_buffered_events()
	_mouse_button(b, false)
	await _settle()


func _shot(path: String) -> void:
	for i in 4:
		await get_tree().process_frame
	get_viewport().get_texture().get_image().save_png(path)


## Frames drawn over half a second once things settle (tweens, toasts, deferred layout).
func _idle_frames() -> int:
	await get_tree().create_timer(1.5).timeout
	var before := Engine.get_frames_drawn()
	await get_tree().create_timer(0.5).timeout
	return Engine.get_frames_drawn() - before


## Lets deferred layout run, so controls are where a user would see them.
func _settle() -> void:
	await get_tree().process_frame
	await get_tree().process_frame


func _tap(button: Button) -> void:
	var pos := button.get_global_rect().get_center()
	_touch(5, pos, true)
	_touch(5, pos, false)
	await get_tree().process_frame


func _touch_stroke(index: int, from: Vector2, span: Vector2, tilt: Vector2, pressure: float) -> void:
	_touch(index, from, true)
	for i in range(1, 41):
		var ev := InputEventScreenDrag.new()
		ev.index = index
		ev.position = from + span * (i / 40.0)
		ev.tilt = tilt
		ev.pressure = pressure
		Input.parse_input_event(ev)
		if i % 10 == 0:
			await get_tree().process_frame
	_touch(index, from + span, false)
	await get_tree().process_frame


func _touch(index: int, pos: Vector2, pressed: bool) -> void:
	var ev := InputEventScreenTouch.new()
	ev.index = index
	ev.position = pos
	ev.pressed = pressed
	Input.parse_input_event(ev)
	Input.flush_buffered_events()


func _mouse_button(pos: Vector2, pressed: bool) -> void:
	var ev := InputEventMouseButton.new()
	ev.button_index = MOUSE_BUTTON_LEFT
	ev.position = pos
	ev.pressed = pressed
	Input.parse_input_event(ev)
	Input.flush_buffered_events()


func _key(code: Key, cmd := false, shift := false) -> void:
	var ev := InputEventKey.new()
	ev.keycode = code
	ev.pressed = true
	# The app's shortcuts are Cmd on macOS and Ctrl elsewhere (is_command_or_control_pressed).
	ev.meta_pressed = cmd and OS.has_feature("macos")
	ev.ctrl_pressed = cmd and not OS.has_feature("macos")
	ev.shift_pressed = shift
	Input.parse_input_event(ev)
	Input.flush_buffered_events()
