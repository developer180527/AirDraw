class_name ExportDialog
extends Control
## Export dialog generated from option schemas (canvas.export_options), so new formats and
## exporter plugins get a UI without writing any. Writes to the Exports folder until
## a share sheet exists (Files › AirDraw › Exports on iPad, ~/Documents/AirDraw/Exports on desktop).

signal finished(message: String)


var canvas: SketchCanvas
var stem := "sketch"
var _formats: Array[Dictionary] = []
var _format_picker: OptionButton
var _grid: GridContainer
var _controls := {}  ## option name → Control
var _specs := {}  ## option name → schema dictionary
var _remembered := {}  ## exporter id → {option: value}


func _init() -> void:
	set_anchors_preset(Control.PRESET_FULL_RECT)
	visible = false
	var dim := ColorRect.new()
	dim.color = Color(0, 0, 0, 0.35)
	dim.set_anchors_preset(Control.PRESET_FULL_RECT)
	dim.mouse_filter = Control.MOUSE_FILTER_STOP
	add_child(dim)

	var center := CenterContainer.new()
	center.set_anchors_preset(Control.PRESET_FULL_RECT)
	center.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(center)
	var panel := PanelContainer.new()
	center.add_child(panel)
	var box := VBoxContainer.new()
	panel.add_child(box)

	var title := Label.new()
	title.text = "Export"
	title.add_theme_font_size_override("font_size", 22)
	box.add_child(title)
	_format_picker = OptionButton.new()
	_format_picker.item_selected.connect(func(_i: int) -> void: _build_options())
	box.add_child(_format_picker)
	_grid = GridContainer.new()
	_grid.columns = 2
	box.add_child(_grid)

	var buttons := HBoxContainer.new()
	buttons.alignment = BoxContainer.ALIGNMENT_END
	box.add_child(buttons)
	var cancel := Button.new()
	cancel.text = "Cancel"
	cancel.pressed.connect(hide)
	buttons.add_child(cancel)
	var go := Button.new()
	go.text = "Export"
	go.pressed.connect(_export)
	buttons.add_child(go)


func open(p_canvas: SketchCanvas, p_theme: Theme) -> void:
	canvas = p_canvas
	theme = dialog_theme(p_theme)
	var selected := _format_picker.selected
	_formats.assign(canvas.exporters())
	_format_picker.clear()
	for f in _formats:
		_format_picker.add_item("%s (.%s)" % [f.name, f.extension])
	_format_picker.select(clampi(selected, 0, _formats.size() - 1))
	_build_options()
	show()


## The app theme (already scaled) with dialog spacing.
static func dialog_theme(base: Theme) -> Theme:
	var t: Theme = base.duplicate()
	var s := t.default_font_size / 15.0
	t.set_constant("h_separation", "GridContainer", roundi(16 * s))
	t.set_constant("v_separation", "GridContainer", roundi(8 * s))
	t.set_constant("separation", "VBoxContainer", roundi(12 * s))
	var panel: StyleBoxFlat = t.get_stylebox("panel", "PanelContainer").duplicate()
	panel.set_content_margin_all(18 * s)
	t.set_stylebox("panel", "PanelContainer", panel)
	return t


## Option values as currently set in the dialog.
func current_options() -> Dictionary:
	var out := {}
	for name in _controls:
		var c: Control = _controls[name]
		if c is CheckBox:
			out[name] = c.button_pressed
		elif c is SpinBox:
			out[name] = int(c.value) if _specs[name].kind == "int" else c.value
		elif c is ColorPickerButton:
			out[name] = c.color
		elif c is LineEdit:
			out[name] = c.text
	return out


func _format() -> Dictionary:
	return _formats[_format_picker.selected]


func _build_options() -> void:
	for child in _grid.get_children():
		child.queue_free()
	_controls.clear()
	_specs.clear()
	var saved: Dictionary = _remembered.get(_format().id, {})
	var group := ""
	for spec in canvas.export_options(_format().id):
		if spec.kind == "curve":
			continue
		if spec.group != group:
			group = spec.group
			_add_heading("Scene" if group == "scene" else _format().name)
		var control := _make_control(spec, saved.get(spec.name, spec.default))
		var label := Label.new()
		label.text = spec.name.capitalize()
		label.tooltip_text = spec.doc
		label.mouse_filter = Control.MOUSE_FILTER_PASS
		control.tooltip_text = spec.doc
		_grid.add_child(label)
		_grid.add_child(control)
		_controls[spec.name] = control
		_specs[spec.name] = spec


func _add_heading(text: String) -> void:
	var heading := Label.new()
	heading.text = text
	heading.add_theme_color_override("font_color", UiTheme.ACCENT)
	_grid.add_child(heading)
	_grid.add_child(Control.new())


func _make_control(spec: Dictionary, value: Variant) -> Control:
	match spec.kind:
		"bool":
			var c := CheckBox.new()
			c.button_pressed = value
			return c
		"int", "float":
			var c := SpinBox.new()
			c.step = 1.0 if spec.kind == "int" else 0.001
			if spec["min"] != null:
				c.min_value = spec["min"]
			if spec["max"] != null:
				c.max_value = spec["max"]
			c.value = value
			c.custom_minimum_size.x = 140
			return c
		"color":
			var c := ColorPickerButton.new()
			c.color = value
			c.custom_minimum_size = Vector2(80, 0)
			return c
		_:
			var c := LineEdit.new()
			c.text = str(value)
			return c


func _export() -> void:
	var f := _format()
	var options := current_options()
	_remembered[f.id] = options
	var path := AppPaths.exports().path_join("%s.%s" % [stem, f.extension])
	var result: Dictionary = canvas.export_document(f.id, path, options)
	hide()
	if not result.error.is_empty():
		finished.emit("Export failed: " + result.error)
		return
	var msg := "Exported " + ", ".join(Array(result.files).map(func(p: String) -> String: return p.get_file()))
	if OS.has_feature("pc"):
		msg += "  →  " + path.get_base_dir()
	for w in result.warnings:
		msg += "\n⚠ " + w
	finished.emit(msg)
