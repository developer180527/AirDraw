class_name BrushEditor
extends PanelContainer
## Brush editor: edits a working copy of a brush definition with a live 3D preview. Every
## control is generated from stage schemas (canvas.brush_stage_schema), so new stages get
## editor UI automatically. Saves into the user brush folder via the canvas.

signal closed
signal status(message: String)

const LIST_SLOTS := {"filters": "filter", "dynamics": "dynamics stage"}

var canvas: SketchCanvas
## Working copy, shaped like canvas.brush_definition().
var def := {}
var _original_id := ""
var _s := 1.0
var _content: VBoxContainer
var _name_edit: LineEdit
var _error: Label
var _delete: Button
var _preview: MeshInstance3D
var _preview_pending := false


func _init() -> void:
	var root := VBoxContainer.new()
	add_child(root)

	var header := HBoxContainer.new()
	root.add_child(header)
	var title := Label.new()
	title.text = "Brush Editor"
	title.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	header.add_child(title)
	header.add_child(_button("Close", close))

	var view_box := SubViewportContainer.new()
	view_box.stretch = true
	root.add_child(view_box)
	var viewport := SubViewport.new()
	viewport.own_world_3d = true
	viewport.msaa_3d = Viewport.MSAA_2X
	view_box.add_child(viewport)
	var env := WorldEnvironment.new()
	env.environment = Environment.new()
	env.environment.background_mode = Environment.BG_COLOR
	env.environment.background_color = Color(0.95, 0.95, 0.93)
	env.environment.ambient_light_source = Environment.AMBIENT_SOURCE_COLOR
	env.environment.ambient_light_color = Color(0.75, 0.75, 0.8)
	viewport.add_child(env)
	var sun := DirectionalLight3D.new()
	sun.rotation = Vector3(-0.7, 0.6, 0)
	viewport.add_child(sun)
	# Orthographic, framed on the sample stroke (x ±0.4), so widths read true to scale.
	var cam := Camera3D.new()
	cam.projection = Camera3D.PROJECTION_ORTHOGONAL
	cam.size = 0.42
	cam.position = Vector3(0, 0, 2)
	viewport.add_child(cam)
	_preview = MeshInstance3D.new()
	# Slight turn so flat and round geometry both read as 3D.
	_preview.rotation = Vector3(-0.35, 0.35, 0)
	viewport.add_child(_preview)

	var scroll := ScrollContainer.new()
	scroll.size_flags_vertical = Control.SIZE_EXPAND_FILL
	scroll.horizontal_scroll_mode = ScrollContainer.SCROLL_MODE_DISABLED
	root.add_child(scroll)
	_content = VBoxContainer.new()
	_content.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	scroll.add_child(_content)

	_error = Label.new()
	_error.add_theme_color_override("font_color", UiTheme.DANGER)
	_error.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	root.add_child(_error)

	var actions := HFlowContainer.new()
	root.add_child(actions)
	actions.add_child(_button("Save", save))
	actions.add_child(_button("Save as New", save_as_new))
	_delete = _button("Delete", delete)
	actions.add_child(_delete)
	actions.add_child(_button("Share", share))
	actions.add_child(_button("Revert", func() -> void: edit(_original_id)))
	visible = false


func open(p_canvas: SketchCanvas, id: String, base_theme: Theme, top: float) -> void:
	canvas = p_canvas
	theme = ExportDialog.dialog_theme(base_theme)
	_s = theme.default_font_size / 15.0
	set_anchors_preset(Control.PRESET_RIGHT_WIDE)
	offset_left = -440 * _s
	offset_top = top + 8 * _s
	offset_right = -8 * _s
	offset_bottom = -8 * _s
	get_child(0).get_child(1).custom_minimum_size = Vector2(0, 170 * _s)
	show()
	edit(id)


func close() -> void:
	hide()
	closed.emit()


func edit(id: String) -> void:
	_original_id = id
	def = canvas.brush_definition(id).duplicate(true)
	_rebuild()


func save() -> void:
	var was_builtin := canvas.brush_is_builtin(def.id)
	var err: String = canvas.save_brush(def)
	if err.is_empty():
		_original_id = def.id
		_update_delete()
		status.emit("Saved %s%s" % [def.name, " (overrides the built-in; Reset restores it)" if was_builtin else ""])
	_error.text = err


## Saves under a new id so the original stays untouched.
func save_as_new() -> void:
	var name: String = def.name
	if canvas.brush_ids().has(def.id):
		name = name if name != canvas.brush_name(def.id) else name + " Copy"
	var base := "user." + name.to_lower().validate_filename().replace(" ", "_")
	var id := base
	var n := 2
	while canvas.brush_ids().has(id):
		id = "%s_%d" % [base, n]
		n += 1
	def.id = id
	def.name = name
	_name_edit.text = name
	save()


## Deletes a custom brush, or resets an overridden built-in.
func delete() -> void:
	var id: String = def.id
	var err: String = canvas.delete_brush(id)
	if not err.is_empty():
		_error.text = err
		return
	if canvas.brush_ids().has(id):
		status.emit("Reset %s to built-in" % canvas.brush_name(id))
		edit(id)
	else:
		status.emit("Deleted brush")
		close()


func share() -> void:
	var path := AppPaths.exports().path_join("%s.brush.toml" % String(def.id).validate_filename())
	DirAccess.make_dir_recursive_absolute(path.get_base_dir())
	var err: String = canvas.export_brush(def.id, path)
	status.emit("Shared brush → " + path.get_file() if err.is_empty() else "Share failed: " + err)


func refresh_preview() -> void:
	_preview_pending = false
	var result: Dictionary = canvas.brush_preview(def)
	_error.text = result.error
	_preview.visible = result.error.is_empty()
	if result.error.is_empty():
		_preview.mesh = result.mesh
		_preview.material_override = result.material


func _schedule_preview() -> void:
	if not _preview_pending:
		_preview_pending = true
		refresh_preview.call_deferred()


# ---- UI generation --------------------------------------------------------------------

func _rebuild() -> void:
	for child in _content.get_children():
		child.queue_free()
	_name_edit = LineEdit.new()
	_name_edit.text = def.name
	_name_edit.text_changed.connect(func(t: String) -> void: def.name = t)
	_content.add_child(_row("Name", _name_edit))

	var names: Dictionary = canvas.brush_stage_names()
	for slot in LIST_SLOTS:
		_heading(slot.capitalize())
		var stages: Array = def[slot]
		for i in stages.size():
			_list_stage(slot, i)
		var add := OptionButton.new()
		add.add_item("+ Add %s" % LIST_SLOTS[slot])
		for kind in names[slot]:
			add.add_item(kind)
		add.item_selected.connect(func(item: int) -> void:
			if item > 0:
				var kind: String = names[slot][item - 1]
				stages.append({"type": kind, "params": _defaults(slot, kind)})
				_rebuild())
		_content.add_child(add)
	for slot in ["geometry", "material"]:
		_heading(slot.capitalize())
		var picker := OptionButton.new()
		for kind in names[slot]:
			picker.add_item(kind)
			if kind == def[slot].type:
				picker.select(picker.item_count - 1)
		picker.item_selected.connect(func(item: int) -> void:
			var kind: String = names[slot][item]
			def[slot] = {"type": kind, "params": _defaults(slot, kind)}
			_rebuild())
		_content.add_child(picker)
		_params(slot, def[slot])
	_update_delete()
	_schedule_preview()


func _list_stage(slot: String, i: int) -> void:
	var stages: Array = def[slot]
	var stage: Dictionary = stages[i]
	var row := HBoxContainer.new()
	var label := Label.new()
	label.text = String(stage.type).capitalize()
	label.add_theme_color_override("font_color", UiTheme.ACCENT)
	label.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	row.add_child(label)
	var up := _button("Up", func() -> void:
		stages.insert(i - 1, stages.pop_at(i))
		_rebuild())
	up.disabled = i == 0
	row.add_child(up)
	var down := _button("Down", func() -> void:
		stages.insert(i + 1, stages.pop_at(i))
		_rebuild())
	down.disabled = i == stages.size() - 1
	row.add_child(down)
	row.add_child(_button("Remove", func() -> void:
		stages.remove_at(i)
		_rebuild()))
	_content.add_child(row)
	_params(slot, stage)


func _params(slot: String, stage: Dictionary) -> void:
	var params: Dictionary = stage.params
	for spec in canvas.brush_stage_schema(slot, stage.type):
		var name: String = spec.name
		var on_change := func(value: Variant) -> void:
			params[name] = value
			_schedule_preview()
		var control := _control(spec, params.get(name, spec.default), on_change)
		control.tooltip_text = spec.doc
		_content.add_child(_row(name.capitalize(), control, spec.doc))


func _control(spec: Dictionary, value: Variant, on_change: Callable) -> Control:
	match spec.kind:
		"bool":
			var c := CheckBox.new()
			c.button_pressed = value
			c.toggled.connect(on_change)
			return c
		"int":
			var c := SpinBox.new()
			c.min_value = spec["min"]
			c.max_value = spec["max"]
			c.value = value
			c.value_changed.connect(func(v: float) -> void: on_change.call(int(v)))
			return c
		"float":
			var box := HBoxContainer.new()
			var slider := HSlider.new()
			slider.min_value = spec["min"]
			slider.max_value = spec["max"]
			slider.step = 0
			# Ranges like 0.0005–0.5 need a log scale to be usable.
			slider.exp_edit = spec["min"] > 0
			slider.value = value
			slider.size_flags_horizontal = Control.SIZE_EXPAND_FILL
			slider.size_flags_vertical = Control.SIZE_SHRINK_CENTER
			var spin := SpinBox.new()
			spin.min_value = spec["min"]
			spin.max_value = spec["max"]
			spin.step = 0.0001
			spin.value = value
			spin.custom_minimum_size.x = 100 * _s
			slider.value_changed.connect(func(v: float) -> void:
				spin.set_value_no_signal(v)
				on_change.call(v))
			spin.value_changed.connect(func(v: float) -> void:
				slider.set_value_no_signal(v)
				on_change.call(v))
			box.add_child(slider)
			box.add_child(spin)
			return box
		"color":
			var c := ColorPickerButton.new()
			c.color = value
			c.custom_minimum_size = Vector2(80 * _s, 0)
			c.color_changed.connect(on_change)
			return c
		"curve":
			var c := CurveEditor.new()
			c.set_points(value)
			c.custom_minimum_size = Vector2(0, 120 * _s)
			c.changed.connect(on_change)
			return c
		_:
			var c := LineEdit.new()
			c.text = str(value)
			c.text_changed.connect(on_change)
			return c


func _defaults(slot: String, kind: String) -> Dictionary:
	var out := {}
	for spec in canvas.brush_stage_schema(slot, kind):
		out[spec.name] = spec.default
	return out


func _update_delete() -> void:
	var user: bool = canvas.brush_is_user(def.id)
	_delete.disabled = not user
	_delete.text = "Reset" if user and canvas.brush_is_builtin(def.id) else "Delete"


func _heading(text: String) -> void:
	var l := Label.new()
	l.text = text
	l.add_theme_font_size_override("font_size", roundi(18 * _s))
	_content.add_child(l)


## Label above its control: the panel is narrow and sliders/curves need the full width.
func _row(text: String, control: Control, doc := "") -> Control:
	var box := VBoxContainer.new()
	box.add_theme_constant_override("separation", roundi(2 * _s))
	var l := Label.new()
	l.text = text
	l.tooltip_text = doc
	l.mouse_filter = Control.MOUSE_FILTER_PASS
	l.add_theme_color_override("font_color", UiTheme.TEXT_DIM)
	box.add_child(l)
	box.add_child(control)
	return box


func _button(text: String, on_press: Callable) -> Button:
	var b := Button.new()
	b.text = text
	b.focus_mode = Control.FOCUS_NONE
	b.pressed.connect(on_press)
	return b
