class_name UiTheme
## The app's dark chrome around a light canvas. One theme for the editor shell, dialogs,
## sheets and the home screen, scaled so touch targets stay ≥ 44 pt on tablets.

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


## UI scale for a viewport size: 1 at the design height, up to 3 on large screens.
static func scale_for(size: Vector2) -> float:
	return clampf(minf(size.x, size.y) / DESIGN_HEIGHT, 1.0, 3.0)


static func build(s: float) -> Theme:
	var t := Theme.new()
	t.default_font_size = roundi(15 * s)
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

	# Icon-only tool buttons: no background until hovered or active.
	t.set_type_variation("ToolButton", "Button")
	for state in ["normal", "focus"]:
		t.set_stylebox(state, "ToolButton", flat)
	t.set_stylebox("disabled", "ToolButton", flat)

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
	panel.set_corner_radius_all(roundi(14 * s))
	panel.set_content_margin_all(12 * s)
	panel.shadow_color = Color(0, 0, 0, 0.35)
	panel.shadow_size = roundi(10 * s)
	t.set_stylebox("panel", "PanelContainer", panel)
	var popup := panel.duplicate()
	t.set_stylebox("panel", "PopupMenu", popup)
	t.set_color("font_color", "PopupMenu", TEXT)
	t.set_color("font_hover_color", "PopupMenu", Color.WHITE)
	t.set_stylebox("hover", "PopupMenu", _box(ACCENT, roundi(6 * s), s))
	t.set_constant("v_separation", "PopupMenu", roundi(10 * s))

	t.set_constant("separation", "VBoxContainer", roundi(10 * s))
	t.set_constant("separation", "HBoxContainer", roundi(8 * s))
	t.set_constant("h_separation", "GridContainer", roundi(12 * s))
	t.set_constant("v_separation", "GridContainer", roundi(8 * s))
	t.set_constant("h_separation", "HFlowContainer", roundi(8 * s))
	t.set_constant("v_separation", "HFlowContainer", roundi(8 * s))
	t.set_color("font_color", "TooltipLabel", TEXT)
	t.set_stylebox("panel", "TooltipPanel", _box(Color(0.05, 0.05, 0.06), roundi(6 * s), s))
	return t


static func _box(color: Color, radius: int, s: float) -> StyleBoxFlat:
	var b := StyleBoxFlat.new()
	b.bg_color = color
	b.set_corner_radius_all(radius)
	b.content_margin_left = 12 * s
	b.content_margin_right = 12 * s
	b.content_margin_top = 6 * s
	b.content_margin_bottom = 6 * s
	return b
