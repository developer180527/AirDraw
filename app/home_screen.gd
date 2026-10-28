class_name HomeScreen
extends Control
## Launcher shown at startup and when leaving a sketch: New Sketch, Open (system picker), and
## Recents with each file's embedded thumbnail. Not a file manager: organizing, renaming
## and deleting files is the OS's job (Files, Finder, Explorer).
##
## Layout: a header (app mark, title, Open, New Sketch) over a scrolling grid of cards that
## fills the width: the column count follows the screen (3 in portrait and 4–5 in landscape
## on iPad) and the cards stretch to share the row, so the grid never leaves a ragged gap.
## The first card is New Sketch; each recent card has a square preview and a ⋯ menu.

signal new_requested
signal open_requested
signal recent_chosen(entry: Dictionary)
signal remove_requested(entry: Dictionary)
signal reveal_requested(entry: Dictionary)

## Narrowest card at design scale; the columns are as many as fit.
const MIN_CARD := 210.0
const GAP := Vector2(24.0, 30.0)
const SIDE := 36.0
## Content stops growing past this width (very wide windows) and centres.
const MAX_CONTENT := 1680.0
const RADIUS := 14.0
const LONG_PRESS := 0.5
## The thumbnail renderer's background, so empty previews look like blank paper.
const PAPER := Color(0.95, 0.95, 0.93)

## Rounds the corners of a preview to the card's radius (in pixels, set per scale).
const ROUND_SHADER := """
shader_type canvas_item;
uniform float radius = 14.0;
void fragment() {
	vec2 size = 1.0 / max(fwidth(UV), vec2(1e-6));
	vec2 q = abs((UV - 0.5) * size) - (size * 0.5 - radius);
	float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
	COLOR.a *= clamp(0.5 - d, 0.0, 1.0);
}
"""

var _s := 1.0
var _grid: GridContainer
var _new_card: Button
var _count: Label
var _empty: Label
var _status: Label
var _menu: PopupMenu
var _menu_entry := {}
var _long_pressed := false
var _round := ShaderMaterial.new()
var _card_width := 0.0


func _init() -> void:
	set_anchors_preset(Control.PRESET_FULL_RECT)
	mouse_filter = Control.MOUSE_FILTER_STOP
	var bg := ColorRect.new()
	bg.color = UiTheme.BG
	bg.set_anchors_preset(Control.PRESET_FULL_RECT)
	add_child(bg)
	var shader := Shader.new()
	shader.code = ROUND_SHADER
	_round.shader = shader

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
	var mark := TextureRect.new()
	mark.name = "Mark"
	mark.texture = load("res://icon.png")
	mark.expand_mode = TextureRect.EXPAND_IGNORE_SIZE
	mark.stretch_mode = TextureRect.STRETCH_KEEP_ASPECT_CENTERED
	header.add_child(mark)
	var title := Label.new()
	title.name = "Title"
	title.text = "AirDraw"
	title.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	title.vertical_alignment = VERTICAL_ALIGNMENT_CENTER
	header.add_child(title)
	header.add_child(_button("Open", "folder", "SecondaryButton", open_requested.emit))
	header.add_child(_button("New Sketch", "plus", "PrimaryButton", new_requested.emit))

	var rule := HSeparator.new()
	rule.name = "Rule"
	root.add_child(rule)

	_status = Label.new()
	_status.name = "Status"
	_status.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_status.visible = false
	root.add_child(_status)

	var section := HBoxContainer.new()
	section.name = "Section"
	root.add_child(section)
	var recent := Label.new()
	recent.name = "RecentHeading"
	recent.text = "Recent"
	section.add_child(recent)
	_count = Label.new()
	_count.name = "Count"
	_count.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_count.vertical_alignment = VERTICAL_ALIGNMENT_BOTTOM
	section.add_child(_count)

	var scroll := ScrollContainer.new()
	scroll.name = "Scroll"
	scroll.size_flags_vertical = Control.SIZE_EXPAND_FILL
	scroll.horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED
	# Scrolls without a bar (as iPadOS hides it), so the grid always spans the full width.
	scroll.vertical_scroll_mode = ScrollContainer.SCROLL_MODE_SHOW_NEVER
	root.add_child(scroll)
	var content := VBoxContainer.new()
	content.name = "Content"
	content.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	scroll.add_child(content)
	_grid = GridContainer.new()
	_grid.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	content.add_child(_grid)

	_empty = Label.new()
	_empty.text = "Sketches you open or create appear here.\nStart with New Sketch, or Open one from Files."
	_empty.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_empty.add_theme_color_override("font_color", UiTheme.TEXT_DIM)
	content.add_child(_empty)

	_menu = PopupMenu.new()
	_menu.add_item("Open", 2)
	_menu.add_item("Show in Finder" if OS.has_feature("macos") else "Show in Files", 1)
	_menu.add_separator()
	_menu.add_item("Remove from Recents", 0)
	_menu.id_pressed.connect(func(id: int) -> void:
		if id == 0:
			remove_requested.emit(_menu_entry)
		elif id == 1:
			reveal_requested.emit(_menu_entry)
		else:
			recent_chosen.emit(_menu_entry))
	add_child(_menu)
	resized.connect(_update_columns)


## Applies the app's scaled theme (same scale as the canvas toolbar).
func setup(base_theme: Theme) -> void:
	theme = ExportDialog.dialog_theme(base_theme)
	_s = theme.default_font_size / 14.0
	_round.set_shader_parameter("radius", RADIUS * _s)
	$Margin/Root.add_theme_constant_override("separation", roundi(18 * _s))
	var header: HBoxContainer = $Margin/Root/Header
	header.custom_minimum_size.y = roundi(64 * _s)
	header.add_theme_constant_override("separation", roundi(12 * _s))
	var mark: TextureRect = header.get_node("Mark")
	mark.custom_minimum_size = Vector2(36, 36) * _s
	var title: Label = header.get_node("Title")
	title.add_theme_font_override("font", UiTheme.font(700, true))
	title.add_theme_font_size_override("font_size", roundi(26 * _s))
	for b in header.get_children():
		if b is Button:
			b.custom_minimum_size.y = roundi(46 * _s)
			b.size_flags_vertical = Control.SIZE_SHRINK_CENTER
			b.icon = Icons.get_icon(b.get_meta("icon"), _s)
	$Margin/Root/Section.add_theme_constant_override("separation", roundi(10 * _s))
	var heading: Label = $Margin/Root/Section/RecentHeading
	heading.add_theme_font_override("font", UiTheme.font(650, true))
	heading.add_theme_font_size_override("font_size", roundi(20 * _s))
	_count.add_theme_font_size_override("font_size", roundi(13 * _s))
	_count.add_theme_color_override("font_color", UiTheme.TEXT_DIM)
	_status.add_theme_color_override("font_color", UiTheme.DANGER)
	var banner := StyleBoxFlat.new()
	banner.bg_color = Color(UiTheme.DANGER, 0.1)
	banner.border_color = Color(UiTheme.DANGER, 0.35)
	banner.set_border_width_all(maxi(1, roundi(_s)))
	banner.set_corner_radius_all(roundi(10 * _s))
	banner.set_content_margin_all(12 * _s)
	_status.add_theme_stylebox_override("normal", banner)
	_empty.add_theme_font_size_override("font_size", roundi(15 * _s))
	_grid.add_theme_constant_override("h_separation", roundi(GAP.x * _s))
	_grid.add_theme_constant_override("v_separation", roundi(GAP.y * _s))
	$Margin/Root/Scroll/Content.add_theme_constant_override("separation", roundi(24 * _s))
	if _new_card:
		_grid.remove_child(_new_card)
		_new_card.queue_free()
	_new_card = _make_new_card()
	_grid.add_child(_new_card)
	_grid.move_child(_new_card, 0)
	_update_columns()


## `items`: [{entry: Dictionary, texture: Texture2D or null, exists: bool}], newest first.
func show_recents(items: Array) -> void:
	for card in cards():
		_grid.remove_child(card)
		card.queue_free()
	for item in items:
		_grid.add_child(_card(item.entry, item.texture, item.exists))
	_empty.visible = items.is_empty()
	_count.text = "" if items.is_empty() else ("1 sketch" if items.size() == 1 else "%d sketches" % items.size())
	_update_columns()


## The recent cards, newest first (not the New Sketch card).
func cards() -> Array[Button]:
	var out: Array[Button] = []
	for child in _grid.get_children():
		if child != _new_card:
			out.append(child)
	return out


func set_status(text: String) -> void:
	_status.text = text
	_status.visible = not text.is_empty()


## A card: a square tile on top (the caller fills it), name and detail below.
func _card_shell(title: String, detail: String, detail_color: Color) -> Button:
	var card := Button.new()
	card.theme_type_variation = "CardButton"
	card.focus_mode = Control.FOCUS_NONE
	var box := VBoxContainer.new()
	box.name = "Box"
	box.set_anchors_preset(Control.PRESET_FULL_RECT)
	box.add_theme_constant_override("separation", roundi(10 * _s))
	box.mouse_filter = Control.MOUSE_FILTER_IGNORE
	card.add_child(box)

	var tile := Panel.new()
	tile.name = "Tile"
	tile.mouse_filter = Control.MOUSE_FILTER_IGNORE
	box.add_child(tile)
	var ring := Panel.new()
	ring.name = "Ring"
	ring.set_anchors_preset(Control.PRESET_FULL_RECT)
	ring.mouse_filter = Control.MOUSE_FILTER_IGNORE
	var ring_style := StyleBoxFlat.new()
	ring_style.draw_center = false
	ring_style.border_color = UiTheme.ACCENT
	ring_style.set_border_width_all(maxi(2, roundi(2.5 * _s)))
	ring_style.set_corner_radius_all(roundi((RADIUS + 4) * _s))
	ring_style.set_expand_margin_all(4 * _s)
	ring_style.anti_aliasing = true
	ring.add_theme_stylebox_override("panel", ring_style)
	ring.visible = false
	tile.add_child(ring)

	var text := VBoxContainer.new()
	text.name = "Text"
	text.add_theme_constant_override("separation", roundi(2 * _s))
	text.mouse_filter = Control.MOUSE_FILTER_IGNORE
	text.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	var row := HBoxContainer.new()
	row.name = "Row"
	row.mouse_filter = Control.MOUSE_FILTER_IGNORE
	row.add_child(text)
	box.add_child(row)
	var name_label := Label.new()
	name_label.name = "Name"
	name_label.text = title
	name_label.clip_text = true
	name_label.text_overrun_behavior = TextServer.OVERRUN_TRIM_ELLIPSIS
	name_label.add_theme_font_override("font", UiTheme.font(600))
	name_label.add_theme_font_size_override("font_size", roundi(15 * _s))
	name_label.mouse_filter = Control.MOUSE_FILTER_IGNORE
	text.add_child(name_label)
	var info := Label.new()
	info.name = "Detail"
	info.text = detail
	info.add_theme_color_override("font_color", detail_color)
	info.add_theme_font_size_override("font_size", roundi(13 * _s))
	info.mouse_filter = Control.MOUSE_FILTER_IGNORE
	text.add_child(info)

	card.mouse_entered.connect(func() -> void: ring.visible = true)
	card.mouse_exited.connect(func() -> void: ring.visible = false)
	card.button_down.connect(func() -> void: tile.modulate = Color(0.85, 0.85, 0.85))
	card.button_up.connect(func() -> void: tile.modulate = Color.WHITE)
	return card


func _tile_style(color: Color, border: Color) -> StyleBoxFlat:
	var b := StyleBoxFlat.new()
	b.bg_color = color
	b.set_corner_radius_all(roundi(RADIUS * _s))
	b.anti_aliasing = true
	if border.a > 0.0:
		b.set_border_width_all(maxi(1, roundi(1.5 * _s)))
		b.border_color = border
	b.shadow_color = Color(0, 0, 0, 0.35)
	b.shadow_size = roundi(14 * _s)
	b.shadow_offset = Vector2(0, 4 * _s)
	return b


func _make_new_card() -> Button:
	var card := _card_shell("New Sketch", "Blank 3D canvas", UiTheme.TEXT_DIM)
	card.name = "NewSketch"
	card.tooltip_text = "Start a new sketch"
	var tile: Panel = card.get_node("Box/Tile")
	var style := _tile_style(Color(1, 1, 1, 0.035), Color(1, 1, 1, 0.12))
	style.shadow_size = 0
	tile.add_theme_stylebox_override("panel", style)
	var center := CenterContainer.new()
	center.set_anchors_preset(Control.PRESET_FULL_RECT)
	center.mouse_filter = Control.MOUSE_FILTER_IGNORE
	tile.add_child(center)
	var disc := Panel.new()
	disc.custom_minimum_size = Vector2(64, 64) * _s
	disc.mouse_filter = Control.MOUSE_FILTER_IGNORE
	var disc_style := StyleBoxFlat.new()
	disc_style.bg_color = UiTheme.ACCENT
	disc_style.set_corner_radius_all(roundi(32 * _s))
	disc_style.anti_aliasing = true
	disc_style.shadow_color = Color(UiTheme.ACCENT, 0.35)
	disc_style.shadow_size = roundi(16 * _s)
	disc.add_theme_stylebox_override("panel", disc_style)
	center.add_child(disc)
	var plus := TextureRect.new()
	plus.texture = Icons.get_icon("plus", 1.3 * _s)
	plus.stretch_mode = TextureRect.STRETCH_KEEP_CENTERED
	plus.set_anchors_preset(Control.PRESET_FULL_RECT)
	plus.mouse_filter = Control.MOUSE_FILTER_IGNORE
	disc.add_child(plus)
	card.pressed.connect(new_requested.emit)
	return card


func _card(entry: Dictionary, texture: Texture2D, exists: bool) -> Button:
	var card := _card_shell(entry.name, _when(entry.opened_at) if exists else "Not found", UiTheme.TEXT_DIM if exists else UiTheme.DANGER)
	card.tooltip_text = AppPaths.resolve(entry.path).get_file() if entry.path.begins_with(AppPaths.PREFIX) else entry.path
	var tile: Panel = card.get_node("Box/Tile")
	tile.add_theme_stylebox_override("panel", _tile_style(PAPER if exists else Color(1, 1, 1, 0.04), Color(0, 0, 0, 0) if exists else Color(1, 1, 1, 0.1)))
	var thumb := TextureRect.new()
	thumb.name = "Thumb"
	thumb.texture = texture
	thumb.material = _round
	thumb.expand_mode = TextureRect.EXPAND_IGNORE_SIZE
	thumb.stretch_mode = TextureRect.STRETCH_KEEP_ASPECT_CENTERED
	thumb.set_anchors_preset(Control.PRESET_FULL_RECT)
	thumb.mouse_filter = Control.MOUSE_FILTER_IGNORE
	tile.add_child(thumb)
	tile.move_child(thumb, 0)
	if texture == null:
		var note := Label.new()
		note.text = "Empty" if exists else "Missing file"
		note.set_anchors_preset(Control.PRESET_FULL_RECT)
		note.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER
		note.vertical_alignment = VERTICAL_ALIGNMENT_CENTER
		note.add_theme_color_override("font_color", Color(UiTheme.INK, 0.35) if exists else UiTheme.TEXT_DIM)
		note.add_theme_font_size_override("font_size", roundi(13 * _s))
		note.mouse_filter = Control.MOUSE_FILTER_IGNORE
		tile.add_child(note)
		tile.move_child(note, 1)

	var more := Button.new()
	more.name = "More"
	more.theme_type_variation = "ToolButton"
	more.focus_mode = Control.FOCUS_NONE
	more.icon = Icons.get_icon("more", _s)
	more.tooltip_text = "More"
	more.custom_minimum_size = Vector2(40, 40) * _s
	more.add_theme_color_override("icon_normal_color", UiTheme.TEXT_DIM)
	more.add_theme_constant_override("icon_max_width", roundi(22 * _s))
	more.pressed.connect(func() -> void: _open_menu(more, entry))
	card.get_node("Box/Row").add_child(more)

	card.pressed.connect(func() -> void:
		if _long_pressed:
			_long_pressed = false
		else:
			recent_chosen.emit(entry))
	card.gui_input.connect(func(event: InputEvent) -> void: _card_input(event, card, entry))
	_size_card(card)
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


func _open_menu(anchor: Control, entry: Dictionary) -> void:
	_menu_entry = entry
	var r := anchor.get_global_rect()
	_menu.position = Vector2i(r.get_center()) + get_window().position
	_menu.reset_size()
	_menu.popup()


## Columns from the width; cards stretch so each row fills it exactly.
func _update_columns() -> void:
	var margin: MarginContainer = $Margin
	var side := maxf(SIDE * _s, (size.x - MAX_CONTENT * _s) * 0.5)
	margin.add_theme_constant_override("margin_left", roundi(side))
	margin.add_theme_constant_override("margin_right", roundi(side))
	margin.add_theme_constant_override("margin_top", roundi(20 * _s))
	margin.add_theme_constant_override("margin_bottom", 0)
	var gap := GAP.x * _s
	var available := size.x - 2.0 * side
	var columns := maxi(1, floori((available + gap) / (MIN_CARD * _s + gap)))
	_grid.columns = columns
	_card_width = floorf((available - gap * (columns - 1)) / columns)
	for child in _grid.get_children():
		_size_card(child)
	_empty.custom_minimum_size.x = minf(available, 520 * _s)


func _size_card(card: Control) -> void:
	if _card_width <= 0.0:
		return
	var tile: Control = card.get_node("Box/Tile")
	tile.custom_minimum_size = Vector2(_card_width, _card_width)
	card.custom_minimum_size = Vector2(_card_width, _card_width + 64 * _s)


func _button(text: String, icon: String, variation: String, on_press: Callable) -> Button:
	var b := Button.new()
	b.text = text
	b.name = text
	b.set_meta("icon", icon)
	b.theme_type_variation = variation
	b.focus_mode = Control.FOCUS_NONE
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
