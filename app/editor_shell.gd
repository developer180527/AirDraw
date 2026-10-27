class_name EditorShell
extends CanvasLayer
## The drawing screen's chrome, floating over a full-screen canvas (docs/ARCHITECTURE.md §5-3).
##
## Light round buttons float along the top: Gallery and the sketch name on the left, the tools
## (Draw, Guide) in the middle, undo/redo/export/menu on the right, with the view cube below.
## A slim dark tray docked to the hand-side edge holds the recent brushes as nibs; the brush
## list opens next to it with a live stroke preview per brush and closes when you draw. A
## slim bar at the bottom holds the current tool's options. Same layout on tablet and desktop;
## left-handed mode mirrors the tray and cube.

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
var tray_collapsed := false
## Brush ids, most recent first (the tray; keys 1–9).
var recent_brushes: Array[String] = []

var _s := 1.0
var _theme: Theme
var _root: Control
var _gallery: Button
var _title: Label
var _tools: HBoxContainer  ## top center: tool buttons
var _tool_buttons := {}  ## tool → button
var _actions: HBoxContainer  ## top right: undo, redo, export, menu
var _undo: Button
var _redo: Button
var _rail: PanelContainer  ## the tray at the hand-side edge
var _tray_box: VBoxContainer
var _bar: PanelContainer
var _bar_row: HBoxContainer
var _sheet: PanelContainer  ## the brush list
var _brush_list: VBoxContainer
var _brush_scroll: ScrollContainer
var _brush_buttons := {}  ## brush id → row in the brush list
var _brush_group := ButtonGroup.new()
var _previews: BrushPreviews
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
	_previews = BrushPreviews.new()
	add_child(_previews)
	_previews.setup(canvas)
	_previews.preview_ready.connect(_show_preview)

	_build_top()
	_build_tray()
	_bar = _panel("Bar")
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


## The band the top buttons occupy (sheets and dialogs open below it).
func bar_rect() -> Rect2:
	return _gallery.get_global_rect().merge(_actions.get_global_rect()).merge(_tools.get_global_rect())


## Screen areas covered by chrome; touches starting there must not draw.
func ui_rects() -> Array[Rect2]:
	var rects: Array[Rect2] = []
	if ui_hidden:
		return rects
	if _menu.visible:
		# While the menu is open a touch anywhere just closes it.
		rects.append(_scrim.get_global_rect())
	for c: Control in [_gallery, _title, _tools, _actions, _rail, _bar, _cube, _sheet]:
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
	for id in canvas.brush_ids():
		_previews.get_preview(id)
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


## The brush in tray slot `index` (0-based), or "".
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
	# Names first, then visible labels, so a label never shadows a button's name.
	for by_name in [true, false]:
		var stack: Array[Node] = [_root]
		while not stack.is_empty():
			var n: Node = stack.pop_back()
			if n is Button and not n.is_queued_for_deletion() and (n.name == key if by_name else n.text == key):
				return n
			stack.append_array(n.get_children())
	return null


func close_menu() -> void:
	if _menu and _menu.visible:
		_menu.visible = false
		_scrim.visible = false
		layout_changed.emit.call_deferred()


# ---- Building ----------------------------------------------------------------------------

func _panel(variation := "") -> PanelContainer:
	var p := PanelContainer.new()
	p.theme_type_variation = variation
	_root.add_child(p)
	return p


func _round(name: String, icon: String, tooltip: String, on_press: Callable) -> Button:
	var b := _button(name, icon, "", on_press)
	b.theme_type_variation = "RoundButton"
	b.tooltip_text = tooltip
	return b


func _build_top() -> void:
	_gallery = _round("Gallery", "grid", "Gallery", action.emit.bind("gallery"))
	_root.add_child(_gallery)
	_title = Label.new()
	_title.theme_type_variation = "Title"
	_title.clip_text = true
	_title.text_overrun_behavior = TextServer.OVERRUN_TRIM_ELLIPSIS
	_title.vertical_alignment = VERTICAL_ALIGNMENT_CENTER
	_root.add_child(_title)

	_tools = HBoxContainer.new()
	_root.add_child(_tools)
	var group := ButtonGroup.new()
	for t in TOOLS:
		var b := _round(t[1], t[2], Shortcuts.tooltip(t[1], t[3]), func() -> void: tool_selected.emit(t[0]))
		b.toggle_mode = true
		b.button_group = group
		b.button_pressed = t[0] == tool
		_tool_buttons[t[0]] = b
		_tools.add_child(b)

	_actions = HBoxContainer.new()
	_root.add_child(_actions)
	_undo = _round("Undo", "undo", Shortcuts.tooltip("Undo", "undo"), action.emit.bind("undo"))
	_redo = _round("Redo", "redo", Shortcuts.tooltip("Redo", "redo"), action.emit.bind("redo"))
	for b in [_undo, _redo, _round("Export", "export", Shortcuts.tooltip("Export", "export"), action.emit.bind("export")),
			_round("More", "more", "More", func() -> void: _open_menu())]:
		_actions.add_child(b)


func _build_tray() -> void:
	_rail = _panel("TrayLeft")
	_tray_box = VBoxContainer.new()
	_rail.add_child(_tray_box)


func _build_sheet() -> void:
	_sheet = _panel()
	_sheet.name = "BrushSheet"
	var box := VBoxContainer.new()
	_sheet.add_child(box)
	var header := HBoxContainer.new()
	var title := Label.new()
	title.text = "Brushes"
	title.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	title.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	header.add_child(title)
	var close := _button("Close brushes", "chevron_down", "", close_sheet)
	close.theme_type_variation = "ToolButton"
	header.add_child(close)
	box.add_child(header)
	box.add_child(HSeparator.new())
	_brush_scroll = ScrollContainer.new()
	_brush_scroll.horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED
	_brush_list = VBoxContainer.new()
	_brush_list.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_brush_list.add_theme_constant_override("separation", 0)
	_brush_scroll.add_child(_brush_list)
	box.add_child(_brush_scroll)
	box.add_child(HSeparator.new())
	var footer := HBoxContainer.new()
	footer.add_child(_button("Edit Brush", "", "Edit Brush", action.emit.bind("edit_brush")))
	footer.add_child(_button("Import", "", "Import", action.emit.bind("import_brushes")))
	box.add_child(footer)


func _build_toast() -> void:
	_toast = PanelContainer.new()
	_toast.theme_type_variation = "Bar"
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
	col.add_theme_constant_override("separation", 0)
	_menu.add_child(col)
	var item := func(name: String, text: String, on_press: Callable) -> Button:
		var b := _button(name, "", text, func() -> void:
			close_menu()
			on_press.call())
		b.alignment = HORIZONTAL_ALIGNMENT_LEFT
		b.theme_type_variation = "BrushRow"
		col.add_child(b)
		return b
	_left_handed_item = item.call("Left-handed mode", "Left-handed mode", func() -> void:
		left_handed = not left_handed
		_save_settings()
		_layout())
	var hide_item: Button = item.call("Hide interface", "Hide interface", toggle_ui)
	hide_item.tooltip_text = Shortcuts.tooltip("Hide interface", "toggle_ui")
	col.add_child(HSeparator.new())
	item.call("Import brushes", "Import brushes", action.emit.bind("import_brushes"))
	item.call("Clear sketch", "Clear sketch", action.emit.bind("clear"))


func _open_menu() -> void:
	if _menu.visible:
		close_menu()
		return
	_left_handed_item.text = ("✓  " if left_handed else "") + "Left-handed mode"
	_apply_sizes()
	_menu.reset_size()
	var anchor := find_button("More").get_global_rect()
	_menu.position = Vector2(anchor.end.x - _menu.size.x, anchor.end.y + 8 * _s)
	_scrim.visible = true
	_menu.visible = true
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


func _bar_button(name: String, text: String, on_press: Callable) -> Button:
	var b := _button(name, "", text, on_press)
	b.theme_type_variation = "BrushRow"
	_bar_row.add_child(b)
	return b


func _draw_bar() -> void:
	var id := canvas.current_brush()
	var brush := _bar_button("Brushes", canvas.brush_name(id), func() -> void:
		if sheet_open:
			close_sheet()
		else:
			open_sheet())
	brush.set_meta("icon_name", _nib(id))
	brush.tooltip_text = "Brush list"
	_bar_row.add_child(_dot_rect(canvas.brush_color(id)))
	_bar_row.move_child(_bar_row.get_child(1), 0)
	_bar_button("Edit current brush", "Edit", action.emit.bind("edit_brush"))
	if _has_drawn_guide:
		_bar_row.add_child(VSeparator.new())
		var on := Label.new()
		on.text = "On guide"
		on.theme_type_variation = "Small"
		_bar_row.add_child(on)
		_bar_button("Remove guide", "Remove", action.emit.bind("remove_guide")).tooltip_text = \
			"Remove the guide and draw on the preset plane"


func _guide_bar() -> void:
	var draw := _bar_button("Draw guide", "Draw", func() -> void: tool_selected.emit("guide"))
	draw.toggle_mode = true
	draw.button_pressed = _guide_drawing
	draw.tooltip_text = "Draw a line: it becomes a surface you can draw on"
	_bar_row.add_child(VSeparator.new())
	var group := ButtonGroup.new()
	for i in GUIDE_PLANES.size():
		var b := _bar_button(GUIDE_PLANES[i], GUIDE_PLANES[i], plane_requested.emit.bind(i))
		b.toggle_mode = true
		b.button_group = group
		b.button_pressed = not _has_drawn_guide and i == _plane
		b.tooltip_text = "Draw on a flat %s plane" % GUIDE_PLANES[i].to_lower() if i < 3 else "Draw on a plane facing you"
	_bar_row.add_child(VSeparator.new())
	if _has_drawn_guide:
		_bar_button("Remove guide", "Remove", action.emit.bind("remove_guide"))
	else:
		var slider := HSlider.new()
		slider.name = "Guide position"
		slider.min_value = -3.0
		slider.max_value = 3.0
		slider.step = 0.01
		slider.value = _offset
		slider.tooltip_text = "Move the plane along its normal (Alt + wheel)"
		slider.custom_minimum_size.x = 130 * _s
		slider.size_flags_vertical = Control.SIZE_SHRINK_CENTER
		slider.value_changed.connect(guide_offset_changed.emit)
		_bar_row.add_child(slider)
		_bar_button("Reset position", "Reset", func() -> void: guide_offset_changed.emit(0.0))


# ---- Brushes -----------------------------------------------------------------------------

func _rebuild_brushes() -> void:
	for b in _brush_buttons.values():
		b.queue_free()
	_brush_buttons.clear()
	for id in canvas.brush_ids():
		var row := _brush_row(id)
		_brush_buttons[id] = row
		_brush_list.add_child(row)
	var ids := canvas.brush_ids()
	recent_brushes.assign(recent_brushes.filter(func(id: String) -> bool: return id in ids))
	for id in ids:
		if recent_brushes.size() >= RECENT_MAX:
			break
		if id not in recent_brushes:
			recent_brushes.append(id)
	_sync_brushes()
	_apply_sizes()


## A brush list row: nib, name with a colour dot, and the brush's own stroke as a preview.
func _brush_row(id: String) -> Button:
	var b := _button(id, "", "", func() -> void: canvas.set_brush(id))
	b.theme_type_variation = "BrushRow"
	b.toggle_mode = true
	b.button_group = _brush_group
	b.tooltip_text = canvas.brush_name(id)
	b.set_meta("row", true)
	var row := HBoxContainer.new()
	row.set_anchors_preset(Control.PRESET_FULL_RECT)
	row.mouse_filter = Control.MOUSE_FILTER_IGNORE
	b.add_child(row)
	var nib := TextureRect.new()
	nib.name = "Nib"
	nib.set_meta("icon_name", _nib(id))
	nib.stretch_mode = TextureRect.STRETCH_KEEP_CENTERED
	nib.mouse_filter = Control.MOUSE_FILTER_IGNORE
	row.add_child(nib)
	var names := VBoxContainer.new()
	names.alignment = BoxContainer.ALIGNMENT_CENTER
	names.mouse_filter = Control.MOUSE_FILTER_IGNORE
	var name_label := Label.new()
	name_label.name = "Name"
	name_label.text = canvas.brush_name(id)
	name_label.clip_text = true
	names.add_child(name_label)
	var colour := HBoxContainer.new()
	colour.mouse_filter = Control.MOUSE_FILTER_IGNORE
	colour.add_child(_dot_rect(canvas.brush_color(id)))
	names.add_child(colour)
	row.add_child(names)
	var preview := TextureRect.new()
	preview.name = "Preview"
	preview.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	preview.expand_mode = TextureRect.EXPAND_IGNORE_SIZE
	preview.stretch_mode = TextureRect.STRETCH_KEEP_ASPECT_CENTERED
	preview.mouse_filter = Control.MOUSE_FILTER_IGNORE
	preview.texture = _previews.get_preview(id)
	row.add_child(preview)
	return b


func _show_preview(id: String, texture: Texture2D) -> void:
	if _brush_buttons.has(id):
		_brush_buttons[id].find_child("Preview", true, false).texture = texture


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
	# The tray: collapse toggle, recent brushes as nibs (current one outlined), the list.
	for c in _tray_box.get_children():
		_tray_box.remove_child(c)
		c.queue_free()
	var fold := _button("Collapse tray", "chevrons_right" if tray_collapsed != left_handed else "chevrons_left", "", func() -> void:
		tray_collapsed = not tray_collapsed
		_save_settings()
		_sync_brushes())
	fold.theme_type_variation = "TrayButton"
	fold.tooltip_text = "Show brushes" if tray_collapsed else "Hide brushes"
	_tray_box.add_child(fold)
	if not tray_collapsed:
		_tray_box.add_child(HSeparator.new())
		for i in recent_brushes.size():
			var id := recent_brushes[i]
			var b := _button("Recent " + id, _nib(id), "", func() -> void: canvas.set_brush(id))
			b.theme_type_variation = "TrayButton"
			b.toggle_mode = true
			b.button_pressed = id == current
			b.tooltip_text = Shortcuts.tooltip(canvas.brush_name(id), "brush_%d" % (i + 1))
			b.set_meta("dot", canvas.brush_color(id))
			_tray_box.add_child(b)
		_tray_box.add_child(HSeparator.new())
		var all := _button("All brushes", "more", "", func() -> void:
			if sheet_open:
				close_sheet()
			else:
				open_sheet())
		all.theme_type_variation = "TrayButton"
		all.tooltip_text = "Brush list"
		_tray_box.add_child(all)
	_rebuild_bar()


func _sync_history() -> void:
	_undo.disabled = not canvas.can_undo()
	_redo.disabled = not canvas.can_redo()


## Nib icon for a brush, from its geometry: fine pen, marker, flat chisel or dots.
func _nib(id: String) -> String:
	var def: Dictionary = canvas.brush_definition(id)
	match def.get("geometry", {}).get("type", ""):
		"ribbon":
			return "nib_flat"
		"dots":
			return "nib_dots"
	for d in def.get("dynamics", []):
		if d.get("type", "") == "width" and float(d.get("params", {}).get("base", 0.0)) >= 0.02:
			return "nib_marker"
	return "nib_pen"


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
	var m := 12 * _s
	var hand_left := not left_handed
	_apply_sizes()

	# Top row: Gallery + name on the left, tools centered, actions on the right.
	_gallery.position = Vector2(m, m)
	_actions.reset_size()
	_actions.position = Vector2(screen.x - m - _actions.size.x, m)
	_tools.reset_size()
	_tools.position = Vector2((screen.x - _tools.size.x) / 2, m)
	_fit_title()
	var top := m + _gallery.size.y + m

	# Tray docked to the hand-side edge; view cube on the other side.
	_rail.theme_type_variation = "TrayLeft" if hand_left else "TrayRight"
	_rail.reset_size()
	_rail.position = Vector2(0.0 if hand_left else screen.x - _rail.size.x, top + 6 * _s)
	var cube_size := 104 * _s
	_cube.size = Vector2(cube_size, cube_size)
	_cube.position = Vector2(screen.x - m - cube_size if hand_left else m, top)

	# Context bar, bottom center.
	_bar.reset_size()
	var bar_w := minf(_bar.size.x, screen.x - 2 * m)
	_bar.size.x = bar_w
	_bar.position = Vector2((screen.x - bar_w) / 2, screen.y - m - _bar.size.y)

	# Brush list next to the tray, sized to its rows up to the room above the bar.
	_sheet.visible = sheet_open
	var w := minf(340 * _s, screen.x - _rail.size.x - 3 * m)
	var room := _bar.position.y - m - _rail.position.y - 120 * _s
	_brush_scroll.custom_minimum_size = Vector2(0, minf(_brush_list.get_combined_minimum_size().y, room))
	_sheet.custom_minimum_size = Vector2(w, 0)
	_sheet.reset_size()
	var sheet_x := _rail.size.x + m if hand_left else screen.x - _rail.size.x - m - w
	_sheet.position = Vector2(sheet_x, _rail.position.y)

	_place_toast()
	layout_changed.emit.call_deferred()


func _place_toast() -> void:
	var screen := get_viewport().get_visible_rect().size
	var m := 12 * _s
	_toast.custom_minimum_size = Vector2(minf(480 * _s, screen.x - 2 * m), 0)
	_toast.reset_size()
	var above := screen.y if ui_hidden else _bar.position.y
	_toast.position = Vector2((screen.x - _toast.size.x) / 2, above - m - _toast.size.y)


## Sizes the sketch name to its text, up to the room left before the tool buttons.
func _fit_title() -> void:
	var font := _title.get_theme_font("font")
	var text_w := font.get_string_size(_title.text, HORIZONTAL_ALIGNMENT_LEFT, -1, _title.get_theme_font_size("font_size")).x
	var x := _gallery.position.x + _gallery.size.x + 10 * _s
	var room := _tools.position.x - x - 12 * _s
	_title.size = Vector2(clampf(text_w + 4 * _s, 0.0, maxf(room, 0.0)), _gallery.size.y)
	_title.position = Vector2(x, _gallery.position.y)


## Touch-friendly sizes and crisp icons at the current scale.
func _apply_sizes() -> void:
	if _root == null:
		return
	var stack: Array[Node] = [_root]
	while not stack.is_empty():
		var n: Node = stack.pop_back()
		stack.append_array(n.get_children())
		if n is Button:
			var v: String = n.theme_type_variation
			n.custom_minimum_size.y = 44 * _s
			if v in ["ToolButton", "RoundButton", "TrayButton"]:
				n.custom_minimum_size.x = 44 * _s
			if n.has_meta("row"):
				n.custom_minimum_size = Vector2(0, 62 * _s)
			if n.has_meta("icon_name"):
				n.icon = Icons.get_icon(n.get_meta("icon_name"), _s)
				n.add_theme_constant_override("icon_max_width", roundi(22 * _s))
			if n.has_meta("dot") and n.get_child_count() == 0:
				# Colour dot in the button's bottom-right corner.
				var dot := _dot_rect(n.get_meta("dot"), 9)
				dot.position = Vector2(44 - 14, 44 - 14) * _s
				n.add_child(dot)
		elif n is TextureRect and n.has_meta("icon_name"):
			n.texture = Icons.get_icon(n.get_meta("icon_name"), _s)
			n.custom_minimum_size = Vector2(44 * _s, 44 * _s)
		elif n is HSlider:
			n.custom_minimum_size.y = 32 * _s
	for row in _brush_buttons.values():
		var label: Label = row.find_child("Name", true, false)
		if label:
			label.custom_minimum_size.x = 104 * _s


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


## A small round colour dot (brush colour), with a light ring so dark colours show on the tray.
func _dot_rect(color: Color, size := 10) -> Panel:
	var p := Panel.new()
	var box := StyleBoxFlat.new()
	box.bg_color = color
	box.set_corner_radius_all(roundi(size * _s))
	box.set_border_width_all(maxi(1, roundi(_s)))
	box.border_color = Color(1, 1, 1, 0.35)
	p.add_theme_stylebox_override("panel", box)
	p.custom_minimum_size = Vector2(size, size) * _s
	p.size = p.custom_minimum_size
	p.size_flags_vertical = Control.SIZE_SHRINK_CENTER
	p.mouse_filter = Control.MOUSE_FILTER_IGNORE
	return p


func _load_settings() -> void:
	var cfg := ConfigFile.new()
	if cfg.load(SETTINGS) == OK:
		left_handed = cfg.get_value("ui", "left_handed", false)
		tray_collapsed = cfg.get_value("ui", "tray_collapsed", false)
		recent_brushes.assign(cfg.get_value("ui", "recent_brushes", []))


func _save_settings() -> void:
	var cfg := ConfigFile.new()
	cfg.load(SETTINGS)
	cfg.set_value("ui", "left_handed", left_handed)
	cfg.set_value("ui", "tray_collapsed", tray_collapsed)
	cfg.set_value("ui", "recent_brushes", recent_brushes)
	cfg.save(SETTINGS)
