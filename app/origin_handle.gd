class_name OriginHandle
extends Control
## Move Origin mode's second handle (docs/ARCHITECTURE.md §3k): a bar beside the origin dot
## that moves the origin along the current plane's own axis (dragging over the canvas moves
## it within the plane). Dragging up moves it toward the viewer's side of the plane. Touch,
## Pencil and mouse. Only the bar takes input.

signal moved  ## its screen area changed (UI regions follow it)

const BAR := Vector2(14.0, 72.0)
## Gap between the origin dot and the bar, in points.
const GAP := 28.0

var canvas: SketchCanvas
var camera: Camera3D
var ui_scale := 1.0
## The plane's normal: the axis the bar moves the origin along.
var normal := Vector3.BACK

var _at := Vector2.ZERO  ## the origin on screen
var _dragging := false
var _start_y := 0.0
var _start_origin := Vector3.ZERO
var _px := 0.0  ## world size of a pixel at the origin's depth


func _init() -> void:
	name = "Origin handle"
	set_anchors_preset(Control.PRESET_FULL_RECT)
	mouse_filter = Control.MOUSE_FILTER_STOP
	visible = false


func _process(_delta: float) -> void:
	if not visible or camera == null or canvas == null:
		return
	var o := canvas.origin()
	if camera.is_position_behind(o):
		return
	var at := camera.unproject_position(o)
	if at.distance_to(_at) > 0.5:
		_at = at
		moved.emit()
		queue_redraw()


func bar_rect() -> Rect2:
	var s := ui_scale
	return Rect2(_at + Vector2(GAP, -BAR.y * 0.5) * s, BAR * s)


func areas() -> Array[Rect2]:
	var out: Array[Rect2] = []
	if visible:
		out.append(bar_rect().grow(8.0 * ui_scale))
	return out


func _has_point(point: Vector2) -> bool:
	return bar_rect().grow(8.0 * ui_scale).has_point(point)


func _gui_input(event: InputEvent) -> void:
	var press: bool = (event is InputEventMouseButton and event.button_index == MOUSE_BUTTON_LEFT) or (event is InputEventScreenTouch and event.index == 0)
	if press:
		_dragging = event.pressed
		if _dragging:
			_start_y = event.position.y
			_start_origin = canvas.origin()
			var depth := -(camera.global_transform.affine_inverse() * _start_origin).z
			_px = (camera.project_position(_at + Vector2(0, 1), depth) - camera.project_position(_at, depth)).length()
		queue_redraw()
		accept_event()
	elif _dragging and (event is InputEventMouseMotion or (event is InputEventScreenDrag and event.index == 0)):
		# Up the screen moves toward the side of the plane facing the camera.
		var toward := normal if normal.dot(camera.global_position - _start_origin) >= 0.0 else -normal
		canvas.set_origin(_start_origin + toward * (_start_y - event.position.y) * _px)
		accept_event()


func _draw() -> void:
	var s := ui_scale
	var bar := bar_rect()
	var color := UiTheme.ACCENT.lightened(0.3) if _dragging else UiTheme.ACCENT
	draw_rect(bar.grow(2.0 * s), Color(1, 1, 1, 0.85))
	draw_rect(bar, color)
	var mid := bar.get_center()
	for d in [-1.0, 1.0]:
		var tip := mid + Vector2(0, d * 26.0 * s)
		draw_line(mid, tip, Color.WHITE, 1.5 * s, true)
		draw_line(tip, tip + Vector2(-4, -d * 4) * s, Color.WHITE, 1.5 * s, true)
		draw_line(tip, tip + Vector2(4, -d * 4) * s, Color.WHITE, 1.5 * s, true)
	# A thin leader from the dot to the bar.
	draw_line(_at + Vector2(8, 0) * s, Vector2(bar.position.x, mid.y), Color(UiTheme.ACCENT, 0.6), 1.5 * s, true)
