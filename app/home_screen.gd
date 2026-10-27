class_name HomeScreen
extends Control
## Launcher shown at startup and when leaving a sketch: + New, Open… (system picker), and
## Recents with each file's embedded thumbnail. Not a file manager: organizing, renaming
## and deleting files is the OS's job (Files, Finder, Explorer).

signal new_requested
signal open_requested
signal recent_chosen(entry: Dictionary)
signal remove_requested(entry: Dictionary)
signal reveal_requested(entry: Dictionary)

## Card width at design scale (pixels).
const CARD := 220.0
const LONG_PRESS := 0.5

var _s := 1.0
var _grid: GridContainer
var _empty: Label
var _status: Label
var _menu: PopupMenu
var _menu_entry := {}
var _long_pressed := false


func _init() -> void:
	set_anchors_preset(Control.PRESET_FULL_RECT)
	mouse_filter = Control.MOUSE_FILTER_STOP
	var bg := ColorRect.new()
	bg.color = UiTheme.BG
	bg.set_anchors_preset(Control.PRESET_FULL_RECT)
	add_child(bg)

	var margin := MarginContainer.new()
	margin.name = "Margin"
	margin.set_anchors_preset(Control.PRESET_FULL_RECT)
	add_child(margin)
	var root := VBoxContainer.new()
	root.name = "Root"
	margin.add_child(root)

	var header := HBoxContainer.new()
	header.name = "Header"
	root.add_child(header)
	var title := Label.new()
	title.name = "Title"
	title.text = "AirDraw"
	title.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	header.add_child(title)
	header.add_child(_button("Open…", open_requested.emit))
	header.add_child(_button("+ New", new_requested.emit))

	_status = Label.new()
	_status.add_theme_color_override("font_color", UiTheme.DANGER)
	_status.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_status.visible = false
	root.add_child(_status)

	var recent := Label.new()
	recent.name = "RecentHeading"
	recent.text = "Recent"
	root.add_child(recent)

	var scroll := ScrollContainer.new()
	scroll.size_flags_vertical = Control.SIZE_EXPAND_FILL
	scroll.horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED
	root.add_child(scroll)
	_grid = GridContainer.new()
	_grid.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	scroll.add_child(_grid)

	_empty = Label.new()
	_empty.text = "No sketches yet. Tap + New to start, or Open… a sketch from Files."
	_empty.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
	_empty.add_theme_color_override("font_color", UiTheme.TEXT_DIM)
	root.add_child(_empty)

	_menu = PopupMenu.new()
	_menu.add_item("Remove from Recents", 0)
	_menu.add_item("Show in Finder" if OS.has_feature("macos") else "Show in Files", 1)
	_menu.id_pressed.connect(func(id: int) -> void:
		if id == 0:
			remove_requested.emit(_menu_entry)
		else:
			reveal_requested.emit(_menu_entry))
	add_child(_menu)
	resized.connect(_update_columns)


## Applies the app's scaled theme (same scale as the canvas toolbar).
func setup(base_theme: Theme) -> void:
	theme = ExportDialog.dialog_theme(base_theme)
	_s = theme.default_font_size / 15.0
	var m: MarginContainer = $Margin
	for side in ["left", "right", "top", "bottom"]:
		m.add_theme_constant_override("margin_" + side, roundi(24 * _s))
	$Margin/Root.add_theme_constant_override("separation", roundi(14 * _s))
	$Margin/Root/Header/Title.add_theme_font_size_override("font_size", roundi(34 * _s))
	$Margin/Root/RecentHeading.add_theme_font_size_override("font_size", roundi(20 * _s))
	_grid.add_theme_constant_override("h_separation", roundi(18 * _s))
	_grid.add_theme_constant_override("v_separation", roundi(18 * _s))
	_update_columns()


## `items`: [{entry: Dictionary, texture: Texture2D or null, exists: bool}], newest first.
func show_recents(items: Array) -> void:
	for child in _grid.get_children():
		child.queue_free()
	for item in items:
		_grid.add_child(_card(item.entry, item.texture, item.exists))
	_empty.visible = items.is_empty()
	_update_columns()


func set_status(text: String) -> void:
	_status.text = text
	_status.visible = not text.is_empty()


func _card(entry: Dictionary, texture: Texture2D, exists: bool) -> Control:
	var card := Button.new()
	card.focus_mode = Control.FOCUS_NONE
	card.custom_minimum_size = Vector2(CARD, CARD + 64) * _s
	card.tooltip_text = entry.path
	var box := VBoxContainer.new()
	box.set_anchors_preset(Control.PRESET_FULL_RECT)
	box.offset_left = 8 * _s
	box.offset_right = -8 * _s
	box.offset_top = 8 * _s
	box.offset_bottom = -8 * _s
	box.mouse_filter = Control.MOUSE_FILTER_IGNORE
	card.add_child(box)

	var thumb := TextureRect.new()
	thumb.texture = texture
	thumb.expand_mode = TextureRect.EXPAND_IGNORE_SIZE
	thumb.stretch_mode = TextureRect.STRETCH_KEEP_ASPECT_CENTERED
	thumb.size_flags_vertical = Control.SIZE_EXPAND_FILL
	thumb.mouse_filter = Control.MOUSE_FILTER_IGNORE
	if texture == null:
		thumb.modulate = Color(1, 1, 1, 0)
	box.add_child(thumb)
	var name_label := Label.new()
	name_label.text = entry.name
	name_label.clip_text = true
	name_label.text_overrun_behavior = TextServer.OVERRUN_TRIM_ELLIPSIS
	name_label.mouse_filter = Control.MOUSE_FILTER_IGNORE
	box.add_child(name_label)
	var info := Label.new()
	info.text = _when(entry.opened_at) if exists else "Not found"
	info.add_theme_color_override("font_color", UiTheme.TEXT_DIM if exists else UiTheme.DANGER)
	info.add_theme_font_size_override("font_size", roundi(12 * _s))
	info.mouse_filter = Control.MOUSE_FILTER_IGNORE
	box.add_child(info)

	card.pressed.connect(func() -> void:
		if _long_pressed:
			_long_pressed = false
		else:
			recent_chosen.emit(entry))
	card.gui_input.connect(func(event: InputEvent) -> void: _card_input(event, card, entry))
	return card


## Right-click, or a long press with finger or Pencil, opens the card menu.
func _card_input(event: InputEvent, card: Button, entry: Dictionary) -> void:
	if event is InputEventMouseButton and event.pressed and event.button_index == MOUSE_BUTTON_RIGHT:
		_open_menu(card, entry)
	elif (event is InputEventScreenTouch and event.pressed) or (event is InputEventMouseButton and event.pressed and event.button_index == MOUSE_BUTTON_LEFT):
		_long_pressed = false
		var started := Time.get_ticks_msec()
		card.set_meta("press_started", started)
		await get_tree().create_timer(LONG_PRESS).timeout
		if is_instance_valid(card) and card.get_meta("press_started", -1) == started and card.is_pressed():
			_long_pressed = true
			_open_menu(card, entry)
	elif (event is InputEventScreenTouch or event is InputEventMouseButton) and not event.pressed:
		card.set_meta("press_started", -1)


func _open_menu(card: Control, entry: Dictionary) -> void:
	_menu_entry = entry
	var r := card.get_global_rect()
	_menu.position = Vector2i(r.get_center()) + get_window().position
	_menu.reset_size()
	_menu.popup()


func _update_columns() -> void:
	var gap := 18 * _s
	var available := size.x - 48 * _s
	_grid.columns = maxi(1, floori((available + gap) / (CARD * _s + gap)))


func _button(text: String, on_press: Callable) -> Button:
	var b := Button.new()
	b.text = text
	b.focus_mode = Control.FOCUS_NONE
	b.custom_minimum_size.y = 44
	b.pressed.connect(on_press)
	return b


## "Today 14:03", "Yesterday", or a date.
static func _when(unix: int) -> String:
	var bias: int = Time.get_time_zone_from_system().bias * 60
	var t := Time.get_datetime_dict_from_unix_time(unix + bias)
	var now := Time.get_datetime_dict_from_unix_time(int(Time.get_unix_time_from_system()) + bias)
	var day := func(d: Dictionary) -> int: return int(Time.get_unix_time_from_datetime_dict({"year": d.year, "month": d.month, "day": d.day}) / 86400)
	var diff: int = day.call(now) - day.call(t)
	if diff == 0:
		return "Today %02d:%02d" % [t.hour, t.minute]
	if diff == 1:
		return "Yesterday"
	const MONTHS := ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
	return "%s %d, %d" % [MONTHS[t.month - 1], t.day, t.year]
