class_name EditorShell
extends CanvasLayer
## The drawing screen's chrome, floating over a full-screen canvas (docs/ARCHITECTURE.md §5-3).
##
## Pills in the top corners (Gallery + sketch name; undo, redo, export, menu). A tool rail
## (Draw, Guide) with the recent-brush strip below it, on the hand side. A context bar at the
## bottom with the current tool's options. A view cube in the other top corner. The brush
## library is a sheet (side in landscape, bottom in portrait) that closes when you draw.
## Left-handed mode mirrors the rail, strip and cube. Same layout on tablet and desktop.

signal action(name: String)  ## gallery, export, undo, redo, edit_brush, import_brushes, clear, remove_guide
signal tool_selected(tool: String)  ## draw, guide
signal plane_requested(index: int)  ## GuideController preset index (3 = view-aligned)
signal guide_offset_changed(offset: float)
signal view_requested(view: String)  ## front, back, side, left, top, bottom, 3d
signal orbit_requested(delta: Vector2)
signal layout_changed

const SETTINGS := "user://settings.cfg"
const TOOLS := [["draw", "Draw", "brush", "tool_draw"], ["guide", "Guide", "guide", "tool_guide"]]
const GUIDE_PLANES := ["Front", "Side", "Top", "View"]
const RECENT_MAX := 5

var canvas: SketchCanvas
var left_handed := false
var portrait := false
var tool := "draw"
var ui_hidden := false
var sheet_open := false
## Brush ids, most recent first (the strip; keys 1–9).
var recent_brushes: Array[String] = []

var _s := 1.0
var _theme: Theme
var _root: Control
var _nav: PanelContainer  ## top-left pill: Gallery + sketch name
var _actions: PanelContainer  ## top-right pill: undo, redo, export, menu
var _title: Label
var _undo: Button
var _redo: Button
var _rail: PanelContainer
var _tool_buttons := {}  ## tool → rail button
var _strip: PanelContainer
var _strip_box: VBoxContainer
var _bar: PanelContainer
var _bar_row: HBoxContainer
var _sheet: PanelContainer
var _brush_grid: GridContainer
var _brush_scroll: ScrollContainer
var _brush_buttons := {}  ## brush id → button in the library sheet
var _brush_group := ButtonGroup.new()
var _cube: ViewCube
var _toast: PanelContainer
var _toast_label: Label
var _toast_serial := 0
## The ⋯ menu: an in-canvas panel, not a PopupMenu. Godot popups are separate windows that
## close on mouse clicks outside; on iPad (no mouse emulation) they never closed and kept
## capturing all GUI input. The scrim behind it closes it on any tap or click elsewhere.
var _menu: PanelContainer
var _scrim: Control
var _left_handed_item: Button
var _guide_path: Line2D
## Guide state shown in the context bars.
var _has_drawn_guide := false
var _plane := 0
var _offset := 0.0
var _guide_drawing := false


func setup(p_canvas: SketchCanvas, camera: Camera3D) -> void:
	canvas = p_canvas
	_load_settings()
	_root = Control.new()
	_root.set_anchors_preset(Control.PRESET_FULL_RECT)
	_root.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(_root)

	_build_pills()
	_build_rail()
	_strip = _panel()
	_strip_box = VBoxContainer.new()
	_strip.add_child(_strip_box)
	_bar = _panel()
	_bar_row = HBoxContainer.new()
	_bar.add_child(_bar_row)
	_build_sheet()
	_cube = ViewCube.new()
	_cube.camera = camera
	_cube.tooltip_text = "Tap a face to look at it, drag to orbit, double-tap for 3D"
	_cube.view_requested.connect(view_requested.emit)
	_cube.orbit_requested.connect(orbit_requested.emit)
	_root.add_child(_cube)
	_build_toast()
	_build_menu()
	_guide_path = Line2D.new()
	_guide_path.default_color = Color(1.0, 0.55, 0.1)
	_guide_path.joint_mode = Line2D.LINE_JOINT_ROUND
	_guide_path.begin_cap_mode = Line2D.LINE_CAP_ROUND
	_guide_path.end_cap_mode = Line2D.LINE_CAP_ROUND
	_root.add_child(_guide_path)

	canvas.brushes_changed.connect(_rebuild_brushes)
	canvas.brush_changed.connect(func(id: String) -> void:
		_remember_brush(id)
		_sync_brushes())
	canvas.strokes_changed.connect(func(_n: int) -> void: _sync_history())
	_remember_brush(canvas.current_brush())
	_rebuild_brushes()
	_sync_history()
	get_viewport().size_changed.connect(_layout)
	_layout()


# ---- Public API ------------------------------------------------------------------------

func popup_theme() -> Theme:
	return _theme


## The band the top pills occupy (sheets and dialogs open below it).
func bar_rect() -> Rect2:
	return _nav.get_global_rect().merge(_actions.get_global_rect())


## Screen areas covered by chrome; touches starting there must not draw.
func ui_rects() -> Array[Rect2]:
	var rects: Array[Rect2] = []
	if ui_hidden:
		return rects
	if _menu.visible:
		# While the menu is open a touch anywhere just closes it.
		rects.append(_scrim.get_global_rect())
	for c: Control in [_nav, _actions, _rail, _strip, _bar, _cube, _sheet]:
		if c.visible:
			rects.append(c.get_global_rect())
	return rects


func set_title(text: String, dirty: bool) -> void:
	_title.text = text + ("  •" if dirty else "")
	_fit_title()


## Highlights the current tool and shows its context bar.
func set_tool(p_tool: String) -> void:
	tool = p_tool
	# set_pressed_no_signal doesn't release the group's other buttons, so set each one.
	for t in _tool_buttons:
		_tool_buttons[t].set_pressed_no_signal(t == tool)
	_rebuild_bar()


## Guide state for the context bars: a drawn guide, or the preset plane and its offset.
func refresh_guide(has_drawn: bool, plane: int, offset: float) -> void:
	_has_drawn_guide = has_drawn
	_plane = plane
	_offset = offset
	_rebuild_bar()


## Whether the pen draws a guide (the Guide bar's Draw toggle).
func set_guide_drawing(on: bool) -> void:
	_guide_drawing = on
	var b := find_button("Draw guide")
	if b:
		b.set_pressed_no_signal(on)


## The guide stroke being drawn, in screen pixels (empty clears it).
func show_guide_path(points: PackedVector2Array) -> void:
	_guide_path.points = points
	_guide_path.width = 3 * _s


func open_sheet() -> void:
	sheet_open = true
	_layout()


func close_sheet() -> void:
	if sheet_open:
		sheet_open = false
		_layout()


func toggle_ui() -> void:
	ui_hidden = not ui_hidden
	for c in _root.get_children():
		if c != _toast and c != _guide_path and c != _menu and c != _scrim:
			c.visible = not ui_hidden
	close_menu()
	if ui_hidden:
		layout_changed.emit.call_deferred()
		toast("Interface hidden: %s to show it" % ("four-finger tap or Tab" if DisplayServer.is_touchscreen_available() else "Tab"))
	else:
		_layout()


## The brush in strip slot `index` (0-based), or "".
func recent_brush(index: int) -> String:
	return recent_brushes[index] if index < recent_brushes.size() else ""


## A short message that fades away.
func toast(text: String) -> void:
	if text.is_empty():
		return
	_toast_label.text = text
	_toast.visible = true
	_toast.modulate.a = 1.0
	_place_toast()
	_toast_serial += 1
	var serial := _toast_serial
	await get_tree().create_timer(2.5 + text.length() * 0.02).timeout
	if serial == _toast_serial:
		var tween := create_tween()
		tween.tween_property(_toast, "modulate:a", 0.0, 0.4)
		tween.tween_callback(func() -> void: _toast.visible = serial != _toast_serial)


## Touch devices: a one-time hint on how pen and fingers work.
func first_run_hint() -> void:
	var cfg := ConfigFile.new()
	cfg.load(SETTINGS)
	if cfg.get_value("ui", "hint_shown", false) or not DisplayServer.is_touchscreen_available():
		return
	cfg.set_value("ui", "hint_shown", true)
	cfg.save(SETTINGS)
	toast("Pencil draws. One finger orbits, two fingers pan and zoom. Two-finger tap undoes.")


## A button by name, label, or brush id (tests and tutorials).
func find_button(key: String) -> Button:
	if _brush_buttons.has(key):
		return _brush_buttons[key]
	var stack: Array[Node] = [_root]
	while not stack.is_empty():
		var n: Node = stack.pop_back()
		if n is Button and (n.name == key or n.text == key):
			return n
		stack.append_array(n.get_children())
	return null


# ---- Building ----------------------------------------------------------------------------

func _panel() -> PanelContainer:
	var p := PanelContainer.new()
	_root.add_child(p)
	return p


func _build_pills() -> void:
	_nav = _panel()
	var nav := HBoxContainer.new()
	_nav.add_child(nav)
	var gallery := _button("Gallery", "back", "", action.emit.bind("gallery"))
	gallery.theme_type_variation = "ToolButton"
	nav.add_child(gallery)
	_title = Label.new()
	_title.clip_text = true
	_title.text_overrun_behavior = TextServer.OVERRUN_TRIM_ELLIPSIS
	_title.vertical_alignment = VERTICAL_ALIGNMENT_CENTER
	nav.add_child(_title)

	_actions = _panel()
	var row := HBoxContainer.new()
	_actions.add_child(row)
	_undo = _button("Undo", "undo", "", action.emit.bind("undo"))
	_undo.tooltip_text = Shortcuts.tooltip("Undo", "undo")
	_redo = _button("Redo", "redo", "", action.emit.bind("redo"))
	_redo.tooltip_text = Shortcuts.tooltip("Redo", "redo")
	var export := _button("Export", "export", "", action.emit.bind("export"))
	export.tooltip_text = Shortcuts.tooltip("Export", "export")
	for b in [_undo, _redo, export, _button("More", "more", "", func() -> void: _open_menu())]:
		b.theme_type_variation = "ToolButton"
		row.add_child(b)


func _build_rail() -> void:
	_rail = _panel()
	var col := VBoxContainer.new()
	_rail.add_child(col)
	var group := ButtonGroup.new()
	for t in TOOLS:
		var b := _button(t[1], t[2], "", func() -> void: tool_selected.emit(t[0]))
		b.theme_type_variation = "ToolButton"
		b.toggle_mode = true
		b.button_group = group
		b.button_pressed = t[0] == tool
		b.tooltip_text = Shortcuts.tooltip(t[1], t[3])
		_tool_buttons[t[0]] = b
		col.add_child(b)


func _build_sheet() -> void:
	_sheet = _panel()
	_sheet.name = "BrushSheet"
	var box := VBoxContainer.new()
	_sheet.add_child(box)
	var header := HBoxContainer.new()
	var title := Label.new()
	title.text = "Brushes"
	title.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	header.add_child(title)
	var close := _button("Close brushes", "chevron_down", "", close_sheet)
	close.theme_type_variation = "ToolButton"
	header.add_child(close)
	box.add_child(header)
	_brush_scroll = ScrollContainer.new()
	_brush_scroll.horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED
	_brush_grid = GridContainer.new()
	_brush_grid.columns = 2
	_brush_grid.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_brush_scroll.add_child(_brush_grid)
	box.add_child(_brush_scroll)
	var footer := HBoxContainer.new()
	footer.add_child(_button("Edit Brush", "", "Edit Brush", action.emit.bind("edit_brush")))
	footer.add_child(_button("Import", "", "Import", action.emit.bind("import_brushes")))
	box.add_child(footer)


func _build_toast() -> void:
	_toast = PanelContainer.new()
	_toast.visible = false
	_toast.mouse_filter = Control.MOUSE_FILTER_IGNORE
	_toast_label = Label.new()
	_toast_label.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_toast_label.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	_toast.add_child(_toast_label)
	_root.add_child(_toast)


func _build_menu() -> void:
	_scrim = Control.new()
	_scrim.set_anchors_preset(Control.PRESET_FULL_RECT)
	_scrim.mouse_filter = Control.MOUSE_FILTER_STOP
	_scrim.visible = false
	_scrim.gui_input.connect(func(e: InputEvent) -> void:
		if (e is InputEventMouseButton or e is InputEventScreenTouch) and e.pressed:
			close_menu()
			_scrim.accept_event())
	_root.add_child(_scrim)
	_menu = _panel()
	_menu.name = "Menu"
	_menu.visible = false
	var col := VBoxContainer.new()
	_menu.add_child(col)
	var item := func(name: String, text: String, on_press: Callable) -> Button:
		var b := _button(name, "", text, func() -> void:
			close_menu()
			on_press.call())
		b.alignment = HORIZONTAL_ALIGNMENT_LEFT
		b.flat = true
		col.add_child(b)
		return b
	_left_handed_item = item.call("Left-handed mode", "Left-handed mode", func() -> void:
		left_handed = not left_handed
		_save_settings()
		_layout())
	_left_handed_item.toggle_mode = true
	var hide_item: Button = item.call("Hide interface", "Hide interface", toggle_ui)
	hide_item.tooltip_text = Shortcuts.tooltip("Hide interface", "toggle_ui")
	col.add_child(HSeparator.new())
	item.call("Import brushes", "Import brushes", action.emit.bind("import_brushes"))
	item.call("Clear sketch", "Clear sketch", action.emit.bind("clear"))


func _open_menu() -> void:
	if _menu.visible:
		close_menu()
		return
	_left_handed_item.set_pressed_no_signal(left_handed)
	_left_handed_item.text = ("✓  " if left_handed else "") + "Left-handed mode"
	_apply_sizes()
	_menu.reset_size()
	var anchor := find_button("More").get_global_rect()
	_menu.position = Vector2(anchor.end.x - _menu.size.x, anchor.end.y + 6 * _s)
	_scrim.visible = true
	_menu.visible = true
	layout_changed.emit.call_deferred()


func close_menu() -> void:
	if _menu and _menu.visible:
		_menu.visible = false
		_scrim.visible = false
		layout_changed.emit.call_deferred()


# ---- Context bar -------------------------------------------------------------------------

func _rebuild_bar() -> void:
	if _bar_row == null:
		return
	for c in _bar_row.get_children():
		_bar_row.remove_child(c)
		c.queue_free()
	match tool:
		"draw":
			_draw_bar()
		"guide":
			_guide_bar()
	_apply_sizes()
	_layout.call_deferred()


func _draw_bar() -> void:
	var id := canvas.current_brush()
	var brush := _button("Brushes", "", canvas.brush_name(id), func() -> void:
		if sheet_open:
			close_sheet()
		else:
			open_sheet())
	brush.icon = _swatch(canvas.brush_color(id))
	brush.tooltip_text = "Brush library"
	_bar_row.add_child(brush)
	_bar_row.add_child(_button("Edit current brush", "", "Edit", action.emit.bind("edit_brush")))
	if _has_drawn_guide:
		_bar_row.add_child(VSeparator.new())
		var on := Label.new()
		on.text = "On guide"
		on.add_theme_color_override("font_color", UiTheme.TEXT_DIM)
		_bar_row.add_child(on)
		var remove := _button("Remove guide", "", "Remove", action.emit.bind("remove_guide"))
		remove.tooltip_text = "Remove the guide and draw on the preset plane"
		_bar_row.add_child(remove)


func _guide_bar() -> void:
	var draw := _button("Draw guide", "", "Draw", func() -> void: tool_selected.emit("guide"))
	draw.toggle_mode = true
	draw.button_pressed = _guide_drawing
	draw.tooltip_text = "Draw a line: it becomes a surface you can draw on"
	_bar_row.add_child(draw)
	_bar_row.add_child(VSeparator.new())
	var group := ButtonGroup.new()
	for i in GUIDE_PLANES.size():
		var b := _button(GUIDE_PLANES[i], "", GUIDE_PLANES[i], plane_requested.emit.bind(i))
		b.toggle_mode = true
		b.button_group = group
		b.button_pressed = not _has_drawn_guide and i == _plane
		b.tooltip_text = "Draw on a flat %s plane" % GUIDE_PLANES[i].to_lower() if i < 3 else "Draw on a plane facing you"
		_bar_row.add_child(b)
	_bar_row.add_child(VSeparator.new())
	if _has_drawn_guide:
		_bar_row.add_child(_button("Remove guide", "", "Remove", action.emit.bind("remove_guide")))
	else:
		var slider := HSlider.new()
		slider.name = "Guide position"
		slider.min_value = -3.0
		slider.max_value = 3.0
		slider.step = 0.01
		slider.value = _offset
		slider.tooltip_text = "Move the plane along its normal (Alt + wheel)"
		slider.custom_minimum_size.x = 140 * _s
		slider.size_flags_vertical = Control.SIZE_SHRINK_CENTER
		slider.value_changed.connect(guide_offset_changed.emit)
		_bar_row.add_child(slider)
		_bar_row.add_child(_button("Reset position", "", "Reset", func() -> void: guide_offset_changed.emit(0.0)))


# ---- Brushes -----------------------------------------------------------------------------

func _rebuild_brushes() -> void:
	for b in _brush_buttons.values():
		b.queue_free()
	_brush_buttons.clear()
	for id in canvas.brush_ids():
		var b := _button(id, "", canvas.brush_name(id), func() -> void: canvas.set_brush(id))
		b.toggle_mode = true
		b.button_group = _brush_group
		b.alignment = HORIZONTAL_ALIGNMENT_LEFT
		b.size_flags_horizontal = Control.SIZE_EXPAND_FILL
		b.clip_text = true
		b.icon = _swatch(canvas.brush_color(id))
		_brush_buttons[id] = b
		_brush_grid.add_child(b)
	var ids := canvas.brush_ids()
	recent_brushes.assign(recent_brushes.filter(func(id: String) -> bool: return id in ids))
	for id in ids:
		if recent_brushes.size() >= RECENT_MAX:
			break
		if id not in recent_brushes:
			recent_brushes.append(id)
	_sync_brushes()
	_apply_sizes()


func _remember_brush(id: String) -> void:
	if id.is_empty():
		return
	recent_brushes.erase(id)
	recent_brushes.push_front(id)
	if recent_brushes.size() > RECENT_MAX:
		recent_brushes.resize(RECENT_MAX)
	_save_settings()


func _sync_brushes() -> void:
	var current := canvas.current_brush()
	for id in _brush_buttons:
		_brush_buttons[id].set_pressed_no_signal(id == current)
	# The strip: recent brushes as swatches, current one highlighted.
	for c in _strip_box.get_children():
		_strip_box.remove_child(c)
		c.queue_free()
	for i in recent_brushes.size():
		var id := recent_brushes[i]
		var b := _button("Recent " + id, "", "", func() -> void: canvas.set_brush(id))
		b.theme_type_variation = "ToolButton"
		b.toggle_mode = true
		b.button_pressed = id == current
		b.icon = _dot(canvas.brush_color(id))
		b.tooltip_text = Shortcuts.tooltip(canvas.brush_name(id), "brush_%d" % (i + 1))
		_strip_box.add_child(b)
	var all := _button("All brushes", "more", "", open_sheet)
	all.theme_type_variation = "ToolButton"
	all.tooltip_text = "Brush library"
	_strip_box.add_child(all)
	_rebuild_bar()


func _sync_history() -> void:
	_undo.disabled = not canvas.can_undo()
	_redo.disabled = not canvas.can_redo()


# ---- Layout --------------------------------------------------------------------------------

func _layout() -> void:
	if ui_hidden or _root == null:
		return
	var screen := get_viewport().get_visible_rect().size
	_s = UiTheme.scale_for(screen)
	_theme = UiTheme.build(_s)
	_root.theme = _theme
	_cube.ui_scale = _s
	portrait = screen.y > screen.x or screen.x / _s < 760
	var m := 10 * _s
	var hand_left := not left_handed
	_apply_sizes()

	# Pills in the top corners; everything else starts below them.
	_actions.reset_size()
	_actions.position = Vector2(screen.x - m - _actions.size.x, m)
	_fit_title()
	_nav.position = Vector2(m, m)
	var top := maxf(_nav.size.y, _actions.size.y) + 2 * m

	# Rail and brush strip on the hand side, view cube in the other top corner.
	_rail.reset_size()
	_rail.position = Vector2(m if hand_left else screen.x - m - _rail.size.x, top)
	_strip.custom_minimum_size.x = _rail.size.x
	_strip.reset_size()
	_strip.position = Vector2(_rail.position.x, _rail.position.y + _rail.size.y + m)
	var cube_size := 104 * _s
	_cube.size = Vector2(cube_size, cube_size)
	_cube.position = Vector2(screen.x - m - cube_size if hand_left else m, top)

	# Context bar, bottom center.
	_bar.reset_size()
	var bar_w := minf(_bar.size.x, screen.x - 2 * m)
	_bar.size.x = bar_w
	_bar.position = Vector2((screen.x - bar_w) / 2, screen.y - m - _bar.size.y)

	# Brush library sheet: the side opposite the rail in landscape, above the bar in portrait.
	# It fits its brushes, scrolling past a cap.
	_sheet.visible = sheet_open
	_cube.visible = not (sheet_open and not portrait)
	var cap := minf(screen.y * 0.45, 420 * _s) if portrait else _bar.position.y - m - top - 120 * _s
	_brush_scroll.custom_minimum_size.y = minf(_brush_grid.get_combined_minimum_size().y, cap)
	if portrait:
		_sheet.custom_minimum_size = Vector2(screen.x - 2 * m, 0)
		_sheet.reset_size()
		_sheet.position = Vector2(m, _bar.position.y - m - _sheet.size.y)
	else:
		var w := 300 * _s
		_sheet.custom_minimum_size = Vector2(w, 0)
		_sheet.reset_size()
		_sheet.position = Vector2(screen.x - m - w if hand_left else m, top)

	_place_toast()
	layout_changed.emit.call_deferred()


func _place_toast() -> void:
	var screen := get_viewport().get_visible_rect().size
	var m := 10 * _s
	_toast.custom_minimum_size = Vector2(minf(520 * _s, screen.x - 2 * m), 0)
	_toast.reset_size()
	var above := screen.y if ui_hidden else _bar.position.y
	if sheet_open and portrait and not ui_hidden:
		above = _sheet.position.y
	_toast.position = Vector2((screen.x - _toast.size.x) / 2, above - m - _toast.size.y)


## Sizes the sketch name to its text, up to the room left beside the right-hand pill.
func _fit_title() -> void:
	var font := _title.get_theme_font("font")
	var text_w := font.get_string_size(_title.text, HORIZONTAL_ALIGNMENT_LEFT, -1, _title.get_theme_font_size("font_size")).x
	var screen := get_viewport().get_visible_rect().size
	var room := screen.x - 4 * 10 * _s - _actions.size.x - 44 * _s
	_title.custom_minimum_size.x = clampf(text_w + 12 * _s, 0.0, minf(280 * _s, room))
	_nav.reset_size()


## Touch-friendly sizes and crisp icons at the current scale.
func _apply_sizes() -> void:
	if _root == null:
		return
	var stack: Array[Node] = [_root]
	while not stack.is_empty():
		var n: Node = stack.pop_back()
		stack.append_array(n.get_children())
		if n is Button:
			n.custom_minimum_size.y = 44 * _s
			if n.theme_type_variation == "ToolButton":
				n.custom_minimum_size.x = 44 * _s
			if n.has_meta("icon_name"):
				n.icon = Icons.get_icon(n.get_meta("icon_name"), _s)
				n.add_theme_constant_override("icon_max_width", roundi(24 * _s))
		elif n is HSlider:
			n.custom_minimum_size.y = 32 * _s


func _button(name: String, icon: String, text: String, on_press: Callable) -> Button:
	var b := Button.new()
	b.name = name
	b.text = text
	b.tooltip_text = name
	b.focus_mode = Control.FOCUS_NONE
	if not icon.is_empty():
		b.set_meta("icon_name", icon)
	if on_press.is_valid():
		b.pressed.connect(on_press)
	return b


func _swatch(color: Color) -> ImageTexture:
	var img := Image.create(16, 16, false, Image.FORMAT_RGBA8)
	img.fill(color)
	return ImageTexture.create_from_image(img)


## A round colour dot for the brush strip.
func _dot(color: Color) -> ImageTexture:
	var n := 48
	var img := Image.create(n, n, false, Image.FORMAT_RGBA8)
	var r := n * 0.42
	for y in n:
		for x in n:
			var d := Vector2(x + 0.5 - n / 2.0, y + 0.5 - n / 2.0).length()
			var a := clampf(r - d + 0.5, 0.0, 1.0)
			var ring := clampf(1.5 - absf(d - r + 1.0), 0.0, 1.0) * 0.35
			img.set_pixel(x, y, Color(color.lerp(Color.WHITE, ring), a))
	var tex := ImageTexture.create_from_image(img)
	tex.set_size_override(Vector2i(roundi(24 * _s), roundi(24 * _s)))
	return tex


func _load_settings() -> void:
	var cfg := ConfigFile.new()
	if cfg.load(SETTINGS) == OK:
		left_handed = cfg.get_value("ui", "left_handed", false)
		recent_brushes.assign(cfg.get_value("ui", "recent_brushes", []))


func _save_settings() -> void:
	var cfg := ConfigFile.new()
	cfg.load(SETTINGS)
	cfg.set_value("ui", "left_handed", left_handed)
	cfg.set_value("ui", "recent_brushes", recent_brushes)
	cfg.save(SETTINGS)
