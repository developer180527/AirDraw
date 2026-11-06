class_name GuideController
extends Node3D
## The drawing guide (the surface strokes land on), the floor grid and the sketch's origin
## axes (§3k). The preset planes pass through the sketch's origin; keeps the input session's
## projection surface in sync; shows the drawn 3D guide (owned by the canvas's document) in
## its place when there is one.

signal changed

const GRID_SHADER := preload("res://grid.gdshader")
## The six axis planes, one per view-cube face (§3k): the face's view → the normal facing
## the viewer from that side, and the plane's name. YX is XY seen from behind, and so on.
const PLANES := {
	"front": [Vector3.BACK, "XY"], "back": [Vector3.FORWARD, "YX"],
	"top": [Vector3.UP, "XZ"], "bottom": [Vector3.DOWN, "ZX"],
	"side": [Vector3.RIGHT, "YZ"], "left": [Vector3.LEFT, "ZY"],
}
const GUIDE_COLOR := Color(0.3, 0.45, 0.95)
## Guide opacity is a device preference (how you like to see guides), not part of a sketch:
## a file must never change because someone viewed it differently.
const SETTINGS := "user://settings.cfg"
const DEFAULT_OPACITY := 0.1
## "Nearly opaque": strokes behind the guide stay faintly visible.
const MAX_OPACITY := 0.9

var session: SketchSession
var normal := Vector3.BACK
var origin := Vector3.ZERO
var name_text := "XY"
## The active plane, by its view-cube face ("front" = XY, …).
var plane := "front"

var canvas: SketchCanvas
var _guide: MeshInstance3D
var _floor: MeshInstance3D
var _axes: OriginAxes
var _drawn: MeshInstance3D
var _start_line: MeshInstance3D
var _opacity := DEFAULT_OPACITY
## Writes the opacity a moment after the slider stops, not on every step of a drag.
var _save_timer: Timer


func _init(p_session: SketchSession) -> void:
	session = p_session
	# The floor is the XZ plane through the origin.
	_floor = MeshInstance3D.new()
	_floor.name = "Floor grid"
	var floor_mesh := PlaneMesh.new()
	floor_mesh.size = Vector2(20, 20)
	_floor.mesh = floor_mesh
	_floor.material_override = _grid_material(Color(0.4, 0.4, 0.4, 0.35), Color(0, 0, 0, 0), 0.25)
	add_child(_floor)
	_axes = OriginAxes.new()
	add_child(_axes)

	_guide = MeshInstance3D.new()
	var guide_mesh := PlaneMesh.new()
	guide_mesh.size = Vector2(4, 4)
	_guide.mesh = guide_mesh
	_guide.material_override = _grid_material(GUIDE_COLOR, GUIDE_COLOR, 0.1)
	add_child(_guide)

	var cfg := ConfigFile.new()
	if cfg.load(SETTINGS) == OK:
		_opacity = clampf(cfg.get_value("guide", "opacity", DEFAULT_OPACITY), 0.0, MAX_OPACITY)
	_save_timer = Timer.new()
	_save_timer.one_shot = true
	_save_timer.wait_time = 0.5
	_save_timer.timeout.connect(_save_opacity)
	add_child(_save_timer)
	_apply_opacity()


## Shows the canvas's drawn guide whenever it changes (draw, remove, undo, load).
func attach(p_canvas: SketchCanvas) -> void:
	canvas = p_canvas
	canvas.guide_changed.connect(_show_drawn)
	canvas.world_changed.connect(_follow_origin)
	_show_drawn()
	_follow_origin()


func has_drawn() -> bool:
	return canvas != null and canvas.has_drawn_guide()


## Picks the plane of a view-cube face; replaces a drawn guide (undoable).
func set_plane(view: String) -> void:
	if not PLANES.has(view):
		return
	_drop_drawn()
	plane = view
	normal = PLANES[view][0]
	name_text = PLANES[view][1]
	_apply()


## Slides the plane along its normal by moving the sketch's origin: the planes always pass
## through it (§3k).
func move(amount: float) -> void:
	if canvas:
		canvas.set_origin(canvas.origin() + normal.normalized() * amount)


## Grid line colours that read on `background`: grey on light, light grey on dark.
func set_background(color: Color) -> void:
	var dark := color.get_luminance() < 0.45
	_floor.material_override.set_shader_parameter("line_color", Color(0.85, 0.85, 0.9, 0.22) if dark else Color(0.4, 0.4, 0.4, 0.35))
	_axes.set_background(color)


## The planes, floor and axes follow the sketch's origin (moved, or another sketch loaded).
func _follow_origin() -> void:
	origin = canvas.origin()
	_floor.position = origin
	_axes.set_origin(origin)
	_apply()


## How opaque the guide (drawn or preset) is drawn, 0 (hidden, still drawable) to MAX_OPACITY.
func opacity() -> float:
	return _opacity


func set_opacity(value: float) -> void:
	_opacity = clampf(value, 0.0, MAX_OPACITY)
	_apply_opacity()
	_save_timer.start()


func _exit_tree() -> void:
	if not _save_timer.is_stopped():
		_save_opacity()


func _save_opacity() -> void:
	var cfg := ConfigFile.new()
	cfg.load(SETTINGS)
	cfg.set_value("guide", "opacity", _opacity)
	cfg.save(SETTINGS)


## Fill alpha is the opacity; grid lines stay readable at low opacity and fade out near 0.
func _apply_opacity() -> void:
	var o := opacity()
	var line := minf(1.0, 0.5 + o * 0.5) * clampf(o / 0.05, 0.0, 1.0)
	for node: MeshInstance3D in [_guide, _drawn]:
		if node:
			node.material_override.set_shader_parameter("line_color", Color(GUIDE_COLOR, line))
			node.material_override.set_shader_parameter("fill_color", Color(GUIDE_COLOR, o))


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
		var mat := _grid_material(GUIDE_COLOR, GUIDE_COLOR, 0.25)
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
	_apply_opacity()
	changed.emit()


func _grid_material(line: Color, fill: Color, cell: float) -> ShaderMaterial:
	var mat := ShaderMaterial.new()
	mat.shader = GRID_SHADER
	mat.set_shader_parameter("line_color", line)
	mat.set_shader_parameter("fill_color", fill)
	mat.set_shader_parameter("cell", cell)
	return mat
