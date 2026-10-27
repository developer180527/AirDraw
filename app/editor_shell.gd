class_name EditorShell
extends CanvasLayer
## The drawing screen's chrome: floating buttons, tool rail, panels and toasts over the canvas.
##
## Two floating pills sit in the top corners: Gallery and the sketch name on the left; undo,
## redo, export and the menu on the right. Landscape: tool rail on one side, a fixed column of collapsible panels on the other.
## Portrait (or narrow windows): the panels become a bottom sheet with tabs. Left-handed
## mode swaps the sides. Tools that aren't built yet are present but disabled, so the layout
## is final and later features only switch them on.

signal action(name: String)  ## gallery, export, undo, redo, edit_brush, import_brushes, clear
signal plane_requested(index: int)  ## GuideController preset index (3 = view-aligned)
signal guide_offset_changed(offset: float)
signal view_requested(view: String)  ## front, side, top, 3d
signal guide_draw_toggled(on: bool)  ## the next pen stroke draws a 3D guide
signal layout_changed

const SETTINGS := "user://settings.cfg"
const PANELS := ["Brushes", "Guide", "Layers"]
const TOOLS := [
	["brush", "Brush", true], ["eraser", "Eraser", false], ["select", "Select", false],
	[],
	["guide", "Draw guide", true], ["sphere", "Sphere guide", false], ["cylinder", "Cylinder guide", false],
	[],
	["symmetry", "Symmetry", false], ["snap", "Snap to strokes", false],
]
const GUIDE_PLANES := ["Front", "Side", "Top", "View"]

var canvas: SketchCanvas
var left_handed := false
var portrait := false

var _s := 1.0
var _theme: Theme
var _root: Control
var _nav: PanelContainer  ## top-left pill: Gallery + sketch name
var _actions: PanelContainer  ## top-right pill: undo, redo, export, menu
var _title: Label
var _undo: Button
var _redo: Button
var _rail: PanelContainer
var _column: VBoxContainer
var _sheet: PanelContainer
var _sheet_tabs: HBoxContainer
var _sheet_body: MarginContainer
var _sheet_tab := "Brushes"
var _sheet_open := false
var _views: PanelContainer
var _toast: PanelContainer
var _toast_label: Label
var _toast_serial := 0
var _menu: PopupMenu
var _bodies := {}  ## panel name → content Control (moved between column cards and sheet)
var _cards := {}  ## panel name → card in the column
var _panel_shown := {"Brushes": true, "Guide": true, "Layers": false}
var _collapsed := {}
var _brush_grid: GridContainer
var _brush_buttons := {}
var _brush_group := ButtonGroup.new()
var _plane_buttons: Array[Button] = []
var _plane_group := ButtonGroup.new()
var _offset_slider: HSlider
var _offset_label: Label
var _drawn_row: HBoxContainer
var _guide_path: Line2D


func setup(p_canvas: SketchCanvas) -> void:
	canvas = p_canvas
	_load_settings()
	_root = Control.new()
	_root.set_anchors_preset(Control.PRESET_FULL_RECT)
	_root.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(_root)

	_build_pills()
	_build_rail()
	_build_bodies()
	_column = VBoxContainer.new()
	_root.add_child(_column)
	for name in PANELS:
		_cards[name] = _card(name)
		_column.add_child(_cards[name])
	_build_sheet()
	_build_views()
	_build_toast()
	_build_menu()
	_guide_path = Line2D.new()
	_guide_path.default_color = Color(1.0, 0.55, 0.1)
	_guide_path.joint_mode = Line2D.LINE_JOINT_ROUND
	_guide_path.begin_cap_mode = Line2D.LINE_CAP_ROUND
	_guide_path.end_cap_mode = Line2D.LINE_CAP_ROUND
	_root.add_child(_guide_path)

	canvas.brushes_changed.connect(_rebuild_brushes)
	canvas.brush_changed.connect(func(_id: String) -> void: _sync_brushes())
	canvas.strokes_changed.connect(func(_n: int) -> void: _sync_history())
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
	var rects: Array[Rect2] = [_nav.get_global_rect(), _actions.get_global_rect(), _rail.get_global_rect(), _views.get_global_rect()]
	if portrait:
		rects.append(_sheet.get_global_rect())
	else:
		for name in PANELS:
			if _cards[name].visible:
				rects.append(_cards[name].get_global_rect())
	return rects


func set_title(text: String, dirty: bool) -> void:
	_title.text = text + ("  •" if dirty else "")
	_fit_title()


## Highlights the active preset plane; `index` -1 when a drawn guide is active instead.
func show_plane(index: int) -> void:
	for i in _plane_buttons.size():
		_plane_buttons[i].set_pressed_no_signal(i == index)


func show_drawn_guide(on: bool) -> void:
	_drawn_row.visible = on
	_offset_slider.editable = not on


## Syncs the rail's Draw guide button (it turns off after one guide stroke).
func set_guide_drawing(on: bool) -> void:
	find_button("Draw guide").set_pressed_no_signal(on)


## The guide stroke being drawn, in screen pixels (empty clears it).
func show_guide_path(points: PackedVector2Array) -> void:
	_guide_path.points = points
	_guide_path.width = 3 * _s


func set_guide_offset(offset: float) -> void:
	_offset_slider.set_value_no_signal(offset)
	_offset_label.text = "%+.2f" % offset


## A short message that fades away.
func toast(text: String) -> void:
	if text.is_empty():
		return
	_toast_label.text = text
	_toast.visible = true
	_toast.modulate.a = 1.0
	_toast_serial += 1
	var serial := _toast_serial
	await get_tree().create_timer(2.5 + text.length() * 0.02).timeout
	if serial == _toast_serial:
		var tween := create_tween()
		tween.tween_property(_toast, "modulate:a", 0.0, 0.4)
		tween.tween_callback(func() -> void: _toast.visible = serial != _toast_serial)


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

func _build_pills() -> void:
	_nav = PanelContainer.new()
	_root.add_child(_nav)
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

	_actions = PanelContainer.new()
	_root.add_child(_actions)
	var row := HBoxContainer.new()
	_actions.add_child(row)
	_undo = _button("Undo", "undo", "", action.emit.bind("undo"))
	_redo = _button("Redo", "redo", "", action.emit.bind("redo"))
	for b in [_undo, _redo, _button("Export", "export", "", action.emit.bind("export")),
			_button("More", "more", "", func() -> void: _open_menu())]:
		b.theme_type_variation = "ToolButton"
		row.add_child(b)


func _build_rail() -> void:
	_rail = PanelContainer.new()
	_root.add_child(_rail)
	var col := VBoxContainer.new()
	_rail.add_child(col)
	var tool_group := ButtonGroup.new()
	var guide_group := ButtonGroup.new()
	for t in TOOLS:
		if t.is_empty():
			col.add_child(HSeparator.new())
			continue
		var b := _button(t[1], t[0], "", Callable())
		b.theme_type_variation = "ToolButton"
		b.toggle_mode = true
		b.disabled = not t[2]
		b.tooltip_text = t[1] if t[2] else "%s — coming soon" % t[1]
		if t[0] in ["brush", "eraser", "select"]:
			b.button_group = tool_group
		elif t[0] in ["sphere", "cylinder"]:
			b.button_group = guide_group
		b.button_pressed = t[0] == "brush"
		if t[0] == "guide":
			b.toggled.connect(guide_draw_toggled.emit)
		col.add_child(b)


func _build_bodies() -> void:
	# Brushes
	var brushes := VBoxContainer.new()
	_brush_grid = GridContainer.new()
	_brush_grid.columns = 2
	brushes.add_child(_brush_grid)
	var footer := HBoxContainer.new()
	footer.add_child(_button("Edit Brush", "", "Edit Brush", action.emit.bind("edit_brush")))
	footer.add_child(_button("Import", "", "Import", action.emit.bind("import_brushes")))
	brushes.add_child(footer)
	_bodies["Brushes"] = brushes

	# Guide
	var guide := VBoxContainer.new()
	_drawn_row = HBoxContainer.new()
	var drawn_label := Label.new()
	drawn_label.text = "Drawn guide"
	drawn_label.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_drawn_row.add_child(drawn_label)
	_drawn_row.add_child(_button("Remove guide", "", "Remove", action.emit.bind("remove_guide")))
	_drawn_row.visible = false
	guide.add_child(_drawn_row)
	var planes := HBoxContainer.new()
	for i in GUIDE_PLANES.size():
		var b := _button(GUIDE_PLANES[i], "", GUIDE_PLANES[i], plane_requested.emit.bind(i))
		b.toggle_mode = true
		b.button_group = _plane_group
		b.size_flags_horizontal = Control.SIZE_EXPAND_FILL
		_plane_buttons.append(b)
		planes.add_child(b)
	guide.add_child(planes)
	var pos_row := HBoxContainer.new()
	var pos_label := Label.new()
	pos_label.text = "Position"
	pos_row.add_child(pos_label)
	_offset_label = Label.new()
	_offset_label.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_offset_label.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
	_offset_label.add_theme_color_override("font_color", UiTheme.TEXT_DIM)
	pos_row.add_child(_offset_label)
	pos_row.add_child(_button("Reset", "", "Reset", func() -> void: guide_offset_changed.emit(0.0)))
	guide.add_child(pos_row)
	_offset_slider = HSlider.new()
	_offset_slider.min_value = -3.0
	_offset_slider.max_value = 3.0
	_offset_slider.step = 0.01
	_offset_slider.value_changed.connect(func(v: float) -> void:
		_offset_label.text = "%+.2f" % v
		guide_offset_changed.emit(v))
	guide.add_child(_offset_slider)
	_bodies["Guide"] = guide

	# Layers
	var layers := Label.new()
	layers.text = "Layers are coming in a later update."
	layers.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	layers.add_theme_color_override("font_color", UiTheme.TEXT_DIM)
	_bodies["Layers"] = layers


func _card(name: String) -> PanelContainer:
	var card := PanelContainer.new()
	card.name = name + "Panel"
	var box := VBoxContainer.new()
	card.add_child(box)
	var header := HBoxContainer.new()
	var title := Label.new()
	title.text = name
	title.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	header.add_child(title)
	var fold := _button("Collapse " + name, "chevron_up", "", func() -> void:
		_collapsed[name] = not _collapsed.get(name, false)
		_layout())
	fold.theme_type_variation = "ToolButton"
	header.add_child(fold)
	box.add_child(header)
	return card


func _build_sheet() -> void:
	_sheet = PanelContainer.new()
	_root.add_child(_sheet)
	var box := VBoxContainer.new()
	_sheet.add_child(box)
	_sheet_tabs = HBoxContainer.new()
	box.add_child(_sheet_tabs)
	var group := ButtonGroup.new()
	group.allow_unpress = true
	for name in PANELS:
		var b := _button(name, "", name, func() -> void:
			if _sheet_tab == name and _sheet_open:
				_sheet_open = false
			else:
				_sheet_tab = name
				_sheet_open = true
			_layout())
		b.name = "Tab" + name
		b.toggle_mode = true
		b.button_group = group
		b.size_flags_horizontal = Control.SIZE_EXPAND_FILL
		_sheet_tabs.add_child(b)
	_sheet_body = MarginContainer.new()
	box.add_child(_sheet_body)


func _build_views() -> void:
	_views = PanelContainer.new()
	_root.add_child(_views)
	var row := HBoxContainer.new()
	_views.add_child(row)
	for v in [["F", "front", "Front view"], ["S", "side", "Side view"], ["T", "top", "Top view"], ["3D", "3d", "3D view"]]:
		var b := _button(v[2], "", v[0], view_requested.emit.bind(v[1]))
		b.theme_type_variation = "ToolButton"
		b.tooltip_text = v[2]
		row.add_child(b)


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
	_menu = PopupMenu.new()
	_root.add_child(_menu)
	_menu.id_pressed.connect(func(id: int) -> void:
		match id:
			0, 1, 2:
				_panel_shown[PANELS[id]] = not _panel_shown[PANELS[id]]
				if not portrait:
					_layout()
			10:
				left_handed = not left_handed
				_layout()
			20:
				action.emit("import_brushes")
			21:
				action.emit("clear")
		_save_settings())


func _open_menu() -> void:
	_menu.clear()
	if not portrait:
		for i in PANELS.size():
			_menu.add_check_item(PANELS[i] + " panel", i)
			_menu.set_item_checked(_menu.get_item_index(i), _panel_shown[PANELS[i]])
		_menu.add_separator()
	_menu.add_check_item("Left-handed mode", 10)
	_menu.set_item_checked(_menu.get_item_index(10), left_handed)
	_menu.add_separator()
	_menu.add_item("Import brushes", 20)
	_menu.add_item("Clear sketch", 21)
	_menu.theme = _theme
	_menu.reset_size()
	var anchor := find_button("More").get_global_rect()
	_menu.position = Vector2i(anchor.end.x - _menu.size.x, anchor.end.y + 6 * _s) + get_viewport().get_window().position
	_menu.popup()


# ---- Brushes -----------------------------------------------------------------------------

func _rebuild_brushes() -> void:
	for b in _brush_buttons.values():
		b.queue_free()
	_brush_buttons.clear()
	for id in canvas.brush_ids():
		var b := _button(canvas.brush_name(id), "", canvas.brush_name(id), func() -> void: canvas.set_brush(id))
		b.toggle_mode = true
		b.button_group = _brush_group
		b.alignment = HORIZONTAL_ALIGNMENT_LEFT
		b.size_flags_horizontal = Control.SIZE_EXPAND_FILL
		b.clip_text = true
		b.icon = _swatch(canvas.brush_color(id))
		_brush_buttons[id] = b
		_brush_grid.add_child(b)
	_sync_brushes()
	_apply_sizes()


func _sync_brushes() -> void:
	var current := canvas.current_brush()
	for id in _brush_buttons:
		_brush_buttons[id].set_pressed_no_signal(id == current)


func _sync_history() -> void:
	_undo.disabled = not canvas.can_undo()
	_redo.disabled = not canvas.can_redo()


# ---- Layout --------------------------------------------------------------------------------

func _layout() -> void:
	var screen := get_viewport().get_visible_rect().size
	_s = UiTheme.scale_for(screen)
	_theme = UiTheme.build(_s)
	_root.theme = _theme
	portrait = screen.y > screen.x or screen.x / _s < 760
	var m := 10 * _s
	var rail_w := 64 * _s
	var col_w := 300 * _s
	var rail_left := not left_handed

	_apply_sizes()
	# The pills float in the top corners; everything else starts below them.
	_actions.reset_size()
	_actions.position = Vector2(screen.x - m - _actions.size.x, m)
	_fit_title()
	_nav.position = Vector2(m, m)
	var top_h := maxf(_nav.size.y, _actions.size.y) + m

	_rail.reset_size()
	_rail.position = Vector2(m if rail_left else screen.x - m - _rail.size.x, top_h + m)

	# Panel bodies live in the column (landscape) or the sheet (portrait).
	for name in PANELS:
		var body: Control = _bodies[name]
		if body.get_parent():
			body.get_parent().remove_child(body)
	_column.visible = not portrait
	_sheet.visible = portrait
	if portrait:
		for b in _sheet_tabs.get_children():
			b.set_pressed_no_signal(_sheet_open and b.name == "Tab" + _sheet_tab)
		_sheet_body.visible = _sheet_open
		if _sheet_open:
			_sheet_body.add_child(_bodies[_sheet_tab])
		_sheet.custom_minimum_size = Vector2(screen.x - 2 * m, 0)
		_sheet.reset_size()
		_sheet.position = Vector2(m, screen.y - m - _sheet.size.y)
	else:
		for name in PANELS:
			var card: PanelContainer = _cards[name]
			card.visible = _panel_shown[name]
			var box: VBoxContainer = card.get_child(0)
			var collapsed: bool = _collapsed.get(name, false)
			if not collapsed:
				box.add_child(_bodies[name])
			var fold: Button = box.get_child(0).get_child(1)
			fold.icon = Icons.get_icon("chevron_down" if collapsed else "chevron_up", _s)
			card.custom_minimum_size = Vector2(col_w, 0)
		_column.position = Vector2(screen.x - m - col_w if rail_left else m, top_h + m)
		_column.size = Vector2(col_w, 0)

	_views.reset_size()
	var bottom := (_sheet.position.y if portrait else screen.y) - m
	_views.position = Vector2(m if rail_left else screen.x - m - _views.size.x, bottom - _views.size.y)

	_toast.custom_minimum_size = Vector2(minf(520 * _s, screen.x - 2 * m), 0)
	_toast.reset_size()
	# Above the view switcher row, so the two never overlap on narrow screens.
	_toast.position = Vector2((screen.x - _toast.size.x) / 2, _views.position.y - m - _toast.size.y)

	_apply_sizes()
	layout_changed.emit.call_deferred()


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
	var stack: Array[Node] = [_root, _bodies["Brushes"], _bodies["Guide"]]
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


func _load_settings() -> void:
	var cfg := ConfigFile.new()
	if cfg.load(SETTINGS) == OK:
		left_handed = cfg.get_value("ui", "left_handed", false)
		for name in PANELS:
			_panel_shown[name] = cfg.get_value("ui", "show_" + name.to_lower(), _panel_shown[name])


func _save_settings() -> void:
	var cfg := ConfigFile.new()
	cfg.load(SETTINGS)
	cfg.set_value("ui", "left_handed", left_handed)
	for name in PANELS:
		cfg.set_value("ui", "show_" + name.to_lower(), _panel_shown[name])
	cfg.save(SETTINGS)
