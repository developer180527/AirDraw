class_name ViewCube
extends Control
## A small cube that turns with the camera, so you always see which way you face. Tap a face
## to look straight at it, drag to orbit, double-tap a face to go back to the 3D view.
## Looking straight at a face hides its neighbours, so arrows around the cube show where they
## are; tapping beside the cube in a direction turns to the face that lies that way.

signal view_requested(view: String)  ## front, back, side, left, top, bottom, 3d
signal orbit_requested(delta: Vector2)

## normal, label, view name
const FACES := [
	[Vector3.BACK, "Front", "front"], [Vector3.FORWARD, "Back", "back"],
	[Vector3.RIGHT, "Right", "side"], [Vector3.LEFT, "Left", "left"],
	[Vector3.UP, "Top", "top"], [Vector3.DOWN, "Bottom", "bottom"],
]
const DRAG_THRESHOLD := 6.0
## Faces turned less than this toward the camera are too thin to tap; arrows stand in for them.
const SLIVER := 0.15

var camera: Camera3D
var ui_scale := 1.0

var _basis := Basis()
var _press_pos := Vector2.ZERO
var _pressed := false
var _dragging := false
var _last_tap_ms := 0
var _last_tap_face := ""
var _hover := ""


func _ready() -> void:
	mouse_filter = Control.MOUSE_FILTER_STOP
	mouse_exited.connect(func() -> void:
		_hover = ""
		queue_redraw())


func _process(_delta: float) -> void:
	if camera and not camera.global_transform.basis.is_equal_approx(_basis):
		_basis = camera.global_transform.basis
		queue_redraw()


func _draw() -> void:
	var font := get_theme_font("font", "Label")
	var font_size := roundi(11 * ui_scale)
	for face in _visible_faces():
		var poly: PackedVector2Array = face.poly
		var lit: float = 0.55 + 0.45 * face.facing
		var base := Color(0.33, 0.47, 0.98) if face.view == _hover else Color(0.22, 0.23, 0.27)
		draw_colored_polygon(poly, base.lightened(0.25 * lit) if face.view != _hover else base)
		var outline := poly.duplicate()
		outline.append(poly[0])
		draw_polyline(outline, Color(1, 1, 1, 0.25), maxf(1.0, ui_scale))
		if face.facing > 0.35:
			var w := font.get_string_size(face.label, HORIZONTAL_ALIGNMENT_LEFT, -1, font_size).x
			var c: Vector2 = face.center
			draw_string(font, c + Vector2(-w / 2, font_size * 0.35), face.label, HORIZONTAL_ALIGNMENT_LEFT, -1, font_size,
				Color(1, 1, 1, clampf(face.facing * 1.4, 0.0, 1.0)))
	# Arrows toward faces that are hidden or edge-on.
	var center := size / 2
	var a := 6.0 * ui_scale
	for f in _all_faces():
		if f.facing > SLIVER or f.offset.length() < _radius() * 0.3:
			continue
		var d: Vector2 = f.offset.normalized()
		var tip: Vector2 = center + d * _radius() * 1.38
		var side := Vector2(-d.y, d.x) * a * 0.8
		var col := Color(0.33, 0.47, 0.98) if f.view == _hover else Color(0.3, 0.31, 0.36)
		draw_colored_polygon(PackedVector2Array([tip, tip - d * a + side, tip - d * a - side]), col)


func _gui_input(event: InputEvent) -> void:
	var pressed_event: bool = event is InputEventMouseButton and event.button_index == MOUSE_BUTTON_LEFT or event is InputEventScreenTouch
	if pressed_event:
		if event.pressed:
			_pressed = true
			_dragging = false
			_press_pos = event.position
		elif _pressed:
			_pressed = false
			if not _dragging:
				_tap(event.position)
		accept_event()
	elif event is InputEventMouseMotion or event is InputEventScreenDrag:
		if event is InputEventMouseMotion and not _pressed:
			var face := _face_at(event.position)
			if face != _hover:
				_hover = face
				queue_redraw()
			return
		if _pressed and (_dragging or event.position.distance_to(_press_pos) > DRAG_THRESHOLD * ui_scale):
			_dragging = true
			orbit_requested.emit(event.relative)
		accept_event()


func _tap(pos: Vector2) -> void:
	var face := _face_at(pos)
	var now := Time.get_ticks_msec()
	# A second quick tap on the same face is a double-tap; a tap on another face is just a tap.
	if now - _last_tap_ms < 350 and face == _last_tap_face:
		_last_tap_ms = 0
		view_requested.emit("3d")
		return
	_last_tap_ms = now
	_last_tap_face = face
	if not face.is_empty():
		view_requested.emit(face)


## The view a tap at `pos` asks for: the front-most face under it if that face is turned
## toward the camera enough to aim at, else the face that lies in the tap's direction (so
## from straight on, tapping right of the cube turns to the right face).
func _face_at(pos: Vector2) -> String:
	var faces := _visible_faces()
	faces.reverse()
	for face in faces:
		if face.facing > SLIVER and Geometry2D.is_point_in_polygon(pos, face.poly):
			return face.view
	var toward := pos - size / 2
	if toward.length() < _radius() * 0.25:
		return ""
	var best := ""
	var best_dot := 0.5
	for f in _all_faces():
		if f.facing < -0.3 or f.offset.length() < _radius() * 0.3:
			continue
		var dot: float = f.offset.normalized().dot(toward.normalized())
		if dot > best_dot:
			best_dot = dot
			best = f.view
	return best


func _radius() -> float:
	return minf(size.x, size.y) * 0.34


## Every face with how much it faces the camera and where its center projects, relative to
## the widget's center (zero for the face looked at straight on and the one behind it).
func _all_faces() -> Array:
	var inv := _basis.inverse()
	var out := []
	for f in FACES:
		var c: Vector3 = inv * (f[0] as Vector3)
		out.append({"view": f[2], "facing": c.z, "offset": Vector2(c.x, -c.y) * _radius()})
	return out


## Faces turned toward the camera, back to front, with their screen polygons.
func _visible_faces() -> Array:
	var inv := _basis.inverse()
	var center := size / 2
	var r := _radius()
	var out := []
	for f in FACES:
		var n: Vector3 = f[0]
		var facing := (inv * n).z
		if facing <= 0.02:
			continue
		var u := n.cross(Vector3.UP if absf(n.y) < 0.5 else Vector3.FORWARD).normalized()
		var v := n.cross(u)
		var poly := PackedVector2Array()
		for corner in [u + v, u - v, -u - v, -u + v]:
			var p: Vector3 = inv * (n + corner)
			poly.append(center + Vector2(p.x, -p.y) * r)
		var c3 := inv * n
		out.append({"poly": poly, "facing": facing, "label": f[1], "view": f[2],
			"center": center + Vector2(c3.x, -c3.y) * r, "depth": c3.z})
	out.sort_custom(func(a, b) -> bool: return a.depth < b.depth)
	return out
