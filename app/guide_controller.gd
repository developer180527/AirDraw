class_name GuideController
extends Node3D
## The drawing guide (the surface strokes land on) and the floor grid. Owns the preset
## plane and keeps the input session's projection surface in sync; shows the drawn 3D guide
## (owned by the canvas's document) in its place when there is one.

signal changed

const GRID_SHADER := preload("res://grid.gdshader")
const PLANE_NORMALS := [Vector3.FORWARD, Vector3.RIGHT, Vector3.UP]
const PLANE_NAMES := ["Front (XY)", "Side (YZ)", "Top (XZ)"]
## Index used for the view-aligned plane (matches the toolbar's plane buttons).
const VIEW_ALIGNED := 3

var session: SketchSession
var normal := Vector3.FORWARD
var origin := Vector3.ZERO
var name_text := "Front (XY)"
## Which preset is active (0–2), or VIEW_ALIGNED.
var preset := 0

var canvas: SketchCanvas
var _guide: MeshInstance3D
var _drawn: MeshInstance3D
var _start_line: MeshInstance3D


func _init(p_session: SketchSession) -> void:
	session = p_session
	var floor_grid := MeshInstance3D.new()
	var floor_mesh := PlaneMesh.new()
	floor_mesh.size = Vector2(20, 20)
	floor_grid.mesh = floor_mesh
	floor_grid.material_override = _grid_material(Color(0.4, 0.4, 0.4, 0.35), Color(0, 0, 0, 0), 0.25)
	floor_grid.position.y = -1.0
	add_child(floor_grid)

	_guide = MeshInstance3D.new()
	var guide_mesh := PlaneMesh.new()
	guide_mesh.size = Vector2(4, 4)
	_guide.mesh = guide_mesh
	_guide.material_override = _grid_material(Color(0.3, 0.45, 0.95, 0.5), Color(0.3, 0.45, 0.95, 0.06), 0.1)
	add_child(_guide)


## Shows the canvas's drawn guide whenever it changes (draw, remove, undo, load).
func attach(p_canvas: SketchCanvas) -> void:
	canvas = p_canvas
	canvas.guide_changed.connect(_show_drawn)
	_show_drawn()


func has_drawn() -> bool:
	return canvas != null and canvas.has_drawn_guide()


## Choosing a preset replaces a drawn guide (undoable).
func set_preset(index: int) -> void:
	_drop_drawn()
	preset = index
	normal = PLANE_NORMALS[index]
	name_text = PLANE_NAMES[index]
	_apply()


## Plane facing the camera, through the current origin.
func align_to_view(camera: Camera3D) -> void:
	_drop_drawn()
	preset = VIEW_ALIGNED
	normal = camera.global_transform.basis.z
	name_text = "View-aligned"
	_apply()


## Slides the plane along its normal.
func move(amount: float) -> void:
	origin += normal * amount
	_apply()


## Places the plane at a signed distance along its normal.
func set_offset(value: float) -> void:
	origin = normal.normalized() * value
	_apply()


func reset_origin() -> void:
	origin = Vector3.ZERO
	_apply()


## Signed distance of the plane from the world origin, for display.
func offset() -> float:
	return origin.dot(normal)


func _apply() -> void:
	# PlaneMesh faces +Y; build a basis whose Y axis is the plane normal.
	var y := normal.normalized()
	var x := y.cross(Vector3.UP if absf(y.y) < 0.99 else Vector3.FORWARD).normalized()
	var z := x.cross(y)
	if is_inside_tree():
		_guide.global_transform = Transform3D(Basis(x, y, z), origin)
	else:
		_guide.transform = Transform3D(Basis(x, y, z), origin)
	session.guide_normal = normal
	session.guide_origin = origin
	changed.emit()


func _drop_drawn() -> void:
	if has_drawn():
		canvas.remove_guide()


func _show_drawn() -> void:
	var drawn := has_drawn()
	_guide.visible = not drawn
	if _drawn == null:
		_drawn = MeshInstance3D.new()
		var mat := _grid_material(Color(0.3, 0.45, 0.95, 0.55), Color(0.3, 0.45, 0.95, 0.08), 0.25)
		mat.set_shader_parameter("use_uv", true)
		_drawn.material_override = mat
		add_child(_drawn)
		_start_line = MeshInstance3D.new()
		var line_mat := StandardMaterial3D.new()
		line_mat.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
		line_mat.albedo_color = Color(1.0, 0.55, 0.1)
		_start_line.material_override = line_mat
		add_child(_start_line)
	_drawn.visible = drawn
	_start_line.visible = drawn
	if drawn:
		_drawn.mesh = canvas.guide_mesh()
		var ends: PackedVector3Array = canvas.guide_start_line()
		var line := ImmediateMesh.new()
		line.surface_begin(Mesh.PRIMITIVE_LINES)
		line.surface_add_vertex(ends[0])
		line.surface_add_vertex(ends[1])
		line.surface_end()
		_start_line.mesh = line
	changed.emit()


func _grid_material(line: Color, fill: Color, cell: float) -> ShaderMaterial:
	var mat := ShaderMaterial.new()
	mat.shader = GRID_SHADER
	mat.set_shader_parameter("line_color", line)
	mat.set_shader_parameter("fill_color", fill)
	mat.set_shader_parameter("cell", cell)
	return mat
