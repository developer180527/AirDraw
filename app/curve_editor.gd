class_name CurveEditor
extends Control
## Edits a piecewise-linear curve over 0..1 (e.g. pressure → width). Drag points; tap empty
## space to add one; double-tap/double-click a point to remove it. End points stay at x=0
## and x=1. Works with mouse, touch and Pencil.

signal changed(points: PackedVector2Array)

var points := PackedVector2Array([Vector2(0, 0), Vector2(1, 1)])
var _drag := -1
var _touch_index := -1


func _init() -> void:
	focus_mode = Control.FOCUS_NONE
	mouse_filter = Control.MOUSE_FILTER_STOP


func set_points(p: PackedVector2Array) -> void:
	points = p
	queue_redraw()


func _radius() -> float:
	return maxf(8.0, size.y * 0.06)


func _to_screen(p: Vector2) -> Vector2:
	return Vector2(p.x * size.x, (1.0 - p.y) * size.y)


func _to_curve(s: Vector2) -> Vector2:
	return Vector2(clampf(s.x / size.x, 0, 1), clampf(1.0 - s.y / size.y, 0, 1))


func _draw() -> void:
	draw_rect(Rect2(Vector2.ZERO, size), Color(0.93, 0.94, 0.96))
	var grid := Color(0, 0, 0, 0.08)
	for i in range(1, 4):
		var f := i / 4.0
		draw_line(Vector2(f * size.x, 0), Vector2(f * size.x, size.y), grid)
		draw_line(Vector2(0, f * size.y), Vector2(size.x, f * size.y), grid)
	draw_line(Vector2(0, size.y), Vector2(size.x, 0), Color(0, 0, 0, 0.12))
	var line := PackedVector2Array()
	for p in points:
		line.append(_to_screen(p))
	draw_polyline(line, Color(0.3, 0.45, 0.95), 3.0, true)
	for i in points.size():
		var c := Color(0.3, 0.45, 0.95) if i == _drag else Color(0.15, 0.15, 0.2)
		draw_circle(_to_screen(points[i]), _radius() * 0.55, c)


func _gui_input(event: InputEvent) -> void:
	var pos: Vector2
	var pressed: bool
	var is_double := false
	if event is InputEventMouseButton and event.button_index == MOUSE_BUTTON_LEFT:
		pos = event.position
		pressed = event.pressed
		is_double = event.double_click
	elif event is InputEventScreenTouch and (_touch_index == -1 or event.index == _touch_index):
		pos = event.position
		pressed = event.pressed
		is_double = event.double_tap
		_touch_index = event.index if pressed else -1
	elif (event is InputEventMouseMotion and _drag >= 0) or (event is InputEventScreenDrag and event.index == _touch_index):
		_move(_drag, _to_curve(event.position))
		accept_event()
		return
	else:
		return
	accept_event()
	if not pressed:
		_drag = -1
		queue_redraw()
		return
	var hit := _nearest(pos)
	if hit >= 0 and is_double:
		if hit > 0 and hit < points.size() - 1:
			points.remove_at(hit)
			changed.emit(points)
		queue_redraw()
		return
	if hit < 0:
		hit = _insert(_to_curve(pos))
	_drag = hit
	_move(hit, _to_curve(pos))


func _nearest(screen: Vector2) -> int:
	var best := -1
	var best_d := _radius() * 1.5
	for i in points.size():
		var d := _to_screen(points[i]).distance_to(screen)
		if d < best_d:
			best = i
			best_d = d
	return best


func _insert(p: Vector2) -> int:
	var i := 1
	while i < points.size() - 1 and points[i].x < p.x:
		i += 1
	points.insert(i, p)
	return i


## Moves a point, keeping x order; end points keep their x.
func _move(i: int, p: Vector2) -> void:
	if i < 0:
		return
	if i == 0:
		p.x = 0.0
	elif i == points.size() - 1:
		p.x = 1.0
	else:
		p.x = clampf(p.x, points[i - 1].x + 0.01, points[i + 1].x - 0.01)
	points[i] = p
	queue_redraw()
	changed.emit(points)
