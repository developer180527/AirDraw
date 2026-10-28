class_name UiTheme
## The app's chrome around a light canvas. One theme for the editor shell, dialogs, sheets
## and the home screen, scaled so touch targets stay ≥ 44 pt on tablets.
##
## Two families: light round buttons that float on the canvas (RoundButton), and a dense dark
## tray for tools and brushes (TrayButton, BrushRow, Tray panels), with one accent colour for
## selection.

## Logical layout height the UI is designed for; larger screens scale up.
const DESIGN_HEIGHT := 900.0

const BG := Color(0.11, 0.11, 0.13)
const PANEL := Color(0.15, 0.15, 0.17, 0.97)
const RAISED := Color(0.21, 0.21, 0.24)
const HOVER := Color(0.27, 0.27, 0.31)
const ACCENT := Color(0.33, 0.52, 1.0)
const TEXT := Color(0.92, 0.92, 0.95)
const TEXT_DIM := Color(0.62, 0.62, 0.68)
const DANGER := Color(1.0, 0.45, 0.4)
## Floating round buttons on the canvas.
const LIGHT := Color(0.97, 0.97, 0.96, 0.96)
const LIGHT_HOVER := Color(0.91, 0.91, 0.9, 0.98)
const INK := Color(0.16, 0.17, 0.2)
const TRAY := Color(0.055, 0.055, 0.065, 0.97)
const DIVIDER := Color(1, 1, 1, 0.08)
## Inter (SIL OFL, fonts/Inter-OFL.txt): a variable font, weight 100–900, optical size 14–32.
const FONT := preload("res://fonts/Inter.ttf")

static var _fonts := {}


## Inter at `weight` (400 regular … 700 bold). `display` uses the large optical size, with
## tighter spacing, for titles.
static func font(weight: int, display := false) -> Font:
	var key := weight * 2 + int(display)
	if not _fonts.has(key):
		var f := FontVariation.new()
		f.base_font = FONT
		f.variation_opentype = {TextServerManager.get_primary_interface().name_to_tag("wght"): weight, TextServerManager.get_primary_interface().name_to_tag("opsz"): 32 if display else 14}
		_fonts[key] = f
	return _fonts[key]


## UI scale for a viewport size: 1 at the design height, up to 3 on large screens.
static func scale_for(size: Vector2) -> float:
	return clampf(minf(size.x, size.y) / DESIGN_HEIGHT, 1.0, 3.0)


static func build(s: float) -> Theme:
	var t := Theme.new()
	t.default_font = font(450)
	t.default_font_size = roundi(14 * s)
	var r := roundi(10 * s)

	var normal := _box(RAISED, r, s)
	var hover := _box(HOVER, r, s)
	var pressed := _box(ACCENT, r, s)
	var disabled := _box(Color(RAISED, 0.5), r, s)
	var flat := _box(Color(0, 0, 0, 0), r, s)
	for state in ["normal", "focus"]:
		t.set_stylebox(state, "Button", normal)
	t.set_stylebox("hover", "Button", hover)
	t.set_stylebox("pressed", "Button", pressed)
	t.set_stylebox("hover_pressed", "Button", pressed)
	t.set_stylebox("disabled", "Button", disabled)
	for prop in ["font_color", "font_hover_color", "font_focus_color", "icon_normal_color", "icon_hover_color", "icon_focus_color"]:
		t.set_color(prop, "Button", TEXT)
	for prop in ["font_pressed_color", "font_hover_pressed_color", "icon_pressed_color", "icon_hover_pressed_color"]:
		t.set_color(prop, "Button", Color.WHITE)
	t.set_color("font_disabled_color", "Button", Color(TEXT, 0.3))
	t.set_color("icon_disabled_color", "Button", Color(TEXT, 0.3))
	t.set_constant("h_separation", "Button", roundi(8 * s))
	t.set_font("font", "Button", font(550))

	# Pill buttons: the accent one for the main action, an outlined one beside it.
	var pill := roundi(24 * s)
	t.set_type_variation("PrimaryButton", "Button")
	t.set_stylebox("normal", "PrimaryButton", _pill(ACCENT, pill, s))
	t.set_stylebox("focus", "PrimaryButton", _pill(ACCENT, pill, s))
	t.set_stylebox("hover", "PrimaryButton", _pill(ACCENT.lightened(0.1), pill, s))
	t.set_stylebox("pressed", "PrimaryButton", _pill(ACCENT.darkened(0.15), pill, s))
	t.set_stylebox("hover_pressed", "PrimaryButton", _pill(ACCENT.darkened(0.15), pill, s))
	for prop in ["font_color", "font_hover_color", "font_focus_color", "font_pressed_color", "font_hover_pressed_color", "icon_normal_color", "icon_hover_color", "icon_focus_color", "icon_pressed_color", "icon_hover_pressed_color"]:
		t.set_color(prop, "PrimaryButton", Color.WHITE)
	t.set_type_variation("SecondaryButton", "Button")
	var outlined := _pill(Color(1, 1, 1, 0.0), pill, s)
	outlined.set_border_width_all(maxi(1, roundi(1.2 * s)))
	outlined.border_color = Color(1, 1, 1, 0.16)
	var outlined_hover := outlined.duplicate()
	outlined_hover.bg_color = Color(1, 1, 1, 0.07)
	var outlined_down := outlined.duplicate()
	outlined_down.bg_color = Color(1, 1, 1, 0.13)
	for state in ["normal", "focus"]:
		t.set_stylebox(state, "SecondaryButton", outlined)
	t.set_stylebox("hover", "SecondaryButton", outlined_hover)
	t.set_stylebox("pressed", "SecondaryButton", outlined_down)
	t.set_stylebox("hover_pressed", "SecondaryButton", outlined_down)

	# Whole-card buttons (home screen): the card draws itself.
	t.set_type_variation("CardButton", "Button")
	for state in ["normal", "focus", "hover", "pressed", "hover_pressed", "disabled"]:
		t.set_stylebox(state, "CardButton", StyleBoxEmpty.new())

	# Icon-only tool buttons: no background until hovered or active.
	t.set_type_variation("ToolButton", "Button")
	for state in ["normal", "focus"]:
		t.set_stylebox(state, "ToolButton", flat)
	t.set_stylebox("disabled", "ToolButton", flat)

	# Light round buttons floating on the canvas: dark glyph, soft shadow; accent when active.
	t.set_type_variation("RoundButton", "Button")
	var round_r := roundi(22 * s)
	var round := _round(LIGHT, round_r, s, true)
	for state in ["normal", "focus"]:
		t.set_stylebox(state, "RoundButton", round)
	t.set_stylebox("hover", "RoundButton", _round(LIGHT_HOVER, round_r, s, true))
	t.set_stylebox("pressed", "RoundButton", _round(ACCENT, round_r, s, true))
	t.set_stylebox("hover_pressed", "RoundButton", _round(ACCENT.lightened(0.08), round_r, s, true))
	t.set_stylebox("disabled", "RoundButton", _round(Color(LIGHT, 0.7), round_r, s, true))
	for prop in ["icon_normal_color", "icon_hover_color", "icon_focus_color", "font_color", "font_hover_color", "font_focus_color"]:
		t.set_color(prop, "RoundButton", INK)
	t.set_color("icon_disabled_color", "RoundButton", Color(INK, 0.3))

	# Tray buttons (dark tray): no background; the selected one gets an accent outline.
	t.set_type_variation("TrayButton", "Button")
	var outline := StyleBoxFlat.new()
	outline.bg_color = Color(1, 1, 1, 0.06)
	outline.set_corner_radius_all(roundi(8 * s))
	outline.set_border_width_all(maxi(2, roundi(2 * s)))
	outline.border_color = ACCENT
	outline.set_content_margin_all(4 * s)
	var tray_flat := _box(Color(0, 0, 0, 0), roundi(8 * s), s)
	tray_flat.set_content_margin_all(4 * s)
	var tray_hover := _box(Color(1, 1, 1, 0.07), roundi(8 * s), s)
	tray_hover.set_content_margin_all(4 * s)
	for state in ["normal", "focus", "disabled"]:
		t.set_stylebox(state, "TrayButton", tray_flat)
	t.set_stylebox("hover", "TrayButton", tray_hover)
	t.set_stylebox("pressed", "TrayButton", outline)
	t.set_stylebox("hover_pressed", "TrayButton", outline)

	# Brush list rows: flat; the current brush is filled with the accent.
	t.set_type_variation("BrushRow", "Button")
	var row := _box(Color(0, 0, 0, 0), roundi(6 * s), s)
	var row_hover := _box(Color(1, 1, 1, 0.05), roundi(6 * s), s)
	var row_on := _box(ACCENT, roundi(6 * s), s)
	for state in ["normal", "focus", "disabled"]:
		t.set_stylebox(state, "BrushRow", row)
	t.set_stylebox("hover", "BrushRow", row_hover)
	t.set_stylebox("pressed", "BrushRow", row_on)
	t.set_stylebox("hover_pressed", "BrushRow", row_on)

	# Panels: the dark tray docked at a screen edge (square on that side), the slim bar.
	for side in ["Left", "Right"]:
		var tray := StyleBoxFlat.new()
		tray.bg_color = TRAY
		tray.set_corner_radius_all(roundi(12 * s))
		if side == "Left":
			tray.corner_radius_top_left = 0
			tray.corner_radius_bottom_left = 0
		else:
			tray.corner_radius_top_right = 0
			tray.corner_radius_bottom_right = 0
		tray.set_content_margin_all(6 * s)
		tray.shadow_color = Color(0, 0, 0, 0.25)
		tray.shadow_size = roundi(8 * s)
		t.set_type_variation("Tray" + side, "PanelContainer")
		t.set_stylebox("panel", "Tray" + side, tray)
	var bar := StyleBoxFlat.new()
	bar.bg_color = TRAY
	bar.set_corner_radius_all(roundi(14 * s))
	bar.set_content_margin_all(5 * s)
	bar.shadow_color = Color(0, 0, 0, 0.25)
	bar.shadow_size = roundi(8 * s)
	t.set_type_variation("Bar", "PanelContainer")
	t.set_stylebox("panel", "Bar", bar)
	t.set_type_variation("Title", "Label")
	t.set_color("font_color", "Title", INK)
	t.set_font_size("font_size", "Title", roundi(15 * s))
	t.set_type_variation("Small", "Label")
	t.set_font_size("font_size", "Small", roundi(12 * s))
	t.set_color("font_color", "Small", TEXT_DIM)

	for kind in ["Label", "CheckBox", "OptionButton", "LineEdit", "SpinBox"]:
		t.set_color("font_color", kind, TEXT)
	t.set_color("font_color", "CheckBox", TEXT)
	t.set_color("font_hover_color", "CheckBox", TEXT)
	t.set_color("font_pressed_color", "CheckBox", TEXT)
	var empty := StyleBoxEmpty.new()
	for state in ["normal", "pressed", "hover", "hover_pressed", "focus", "disabled"]:
		t.set_stylebox(state, "CheckBox", empty)

	var field := _box(Color(0.08, 0.08, 0.1), roundi(6 * s), s)
	for state in ["normal", "focus", "read_only"]:
		t.set_stylebox(state, "LineEdit", field)
	t.set_color("font_color", "LineEdit", TEXT)
	t.set_stylebox("normal", "OptionButton", normal)
	t.set_stylebox("hover", "OptionButton", hover)
	t.set_stylebox("pressed", "OptionButton", hover)
	t.set_stylebox("focus", "OptionButton", normal)
	t.set_color("font_color", "OptionButton", TEXT)

	var panel := StyleBoxFlat.new()
	panel.bg_color = PANEL
	panel.set_corner_radius_all(roundi(12 * s))
	panel.set_content_margin_all(10 * s)
	panel.shadow_color = Color(0, 0, 0, 0.28)
	panel.shadow_size = roundi(10 * s)
	t.set_stylebox("panel", "PanelContainer", panel)
	var popup := panel.duplicate()
	popup.set_content_margin_all(6 * s)
	t.set_stylebox("panel", "PopupMenu", popup)
	t.set_color("font_color", "PopupMenu", TEXT)
	t.set_color("font_hover_color", "PopupMenu", Color.WHITE)
	t.set_stylebox("hover", "PopupMenu", _box(ACCENT, roundi(7 * s), s))
	t.set_constant("v_separation", "PopupMenu", roundi(12 * s))
	t.set_constant("item_start_padding", "PopupMenu", roundi(12 * s))
	t.set_constant("item_end_padding", "PopupMenu", roundi(12 * s))

	t.set_constant("separation", "VBoxContainer", roundi(6 * s))
	t.set_constant("separation", "HBoxContainer", roundi(6 * s))
	t.set_color("color", "HSeparator", DIVIDER)
	var line := StyleBoxLine.new()
	line.color = DIVIDER
	line.thickness = maxi(1, roundi(s))
	t.set_stylebox("separator", "HSeparator", line)
	var vline := line.duplicate()
	vline.vertical = true
	t.set_stylebox("separator", "VSeparator", vline)
	t.set_constant("h_separation", "GridContainer", roundi(12 * s))
	t.set_constant("v_separation", "GridContainer", roundi(8 * s))
	t.set_constant("h_separation", "HFlowContainer", roundi(8 * s))
	t.set_constant("v_separation", "HFlowContainer", roundi(8 * s))
	t.set_color("font_color", "TooltipLabel", TEXT)
	t.set_stylebox("panel", "TooltipPanel", _box(Color(0.05, 0.05, 0.06), roundi(6 * s), s))
	return t


## A circle (for a square button) with an optional soft shadow.
static func _round(color: Color, radius: int, s: float, shadow: bool) -> StyleBoxFlat:
	var b := StyleBoxFlat.new()
	b.bg_color = color
	b.set_corner_radius_all(radius)
	b.set_content_margin_all(10 * s)
	b.anti_aliasing = true
	if shadow:
		b.shadow_color = Color(0, 0, 0, 0.18)
		b.shadow_size = roundi(6 * s)
		b.shadow_offset = Vector2(0, 2 * s)
	return b


static func _pill(color: Color, radius: int, s: float) -> StyleBoxFlat:
	var b := StyleBoxFlat.new()
	b.bg_color = color
	b.set_corner_radius_all(radius)
	b.anti_aliasing = true
	b.content_margin_left = 18 * s
	b.content_margin_right = 20 * s
	b.content_margin_top = 10 * s
	b.content_margin_bottom = 10 * s
	return b


static func _box(color: Color, radius: int, s: float) -> StyleBoxFlat:
	var b := StyleBoxFlat.new()
	b.bg_color = color
	b.set_corner_radius_all(radius)
	b.content_margin_left = 12 * s
	b.content_margin_right = 12 * s
	b.content_margin_top = 6 * s
	b.content_margin_bottom = 6 * s
	return b
