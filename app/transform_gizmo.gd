class_name TransformGizmo
extends Control
## Feather-style "joystick" for the selection (docs/ARCHITECTURE.md §3i), drawn at the
## selection's centre, sized in points:
##   centre pad  move in the view plane, at the selection's depth
##   side bar    push / pull along the view direction
##   ring        rotate about the line of sight to the selection
##   top arc     rotate about the world's up axis
##   left arc    rotate about the view's right axis
## While dragging only the stroke nodes move (canvas.preview_selection_transform); releasing
## bakes the move into the strokes as one undo step. Shift snaps rotations to 15°.
## Only the handles take input (_has_point), so the canvas around them still works.

signal moved  ## the gizmo's screen area changed (UI regions follow it)

enum Handle { NONE, MOVE, DEPTH, ROLL, YAW, PITCH }

const RING := 64.0
const PAD := 18.0
const ARC := RING + 16.0
const YAW_ARC := Vector2(-120.0, -60.0)  ## degrees, screen angles (y down: -90 is up)
const PITCH_ARC := Vector2(150.0, 210.0)
const TURN_PER_PX := 0.01
const SNAP := deg_to_rad(15.0)

var canvas: SketchCanvas
var camera: Camera3D
var ui_scale := 1.0

var _center := Vector3.ZERO  ## the selection's centre, world
var _at := Vector2.ZERO  ## where the gizmo is drawn
var _handle := Handle.NONE
var _hover := Handle.NONE
var _start := Vector2.ZERO
var _start_center := Vector2.ZERO
var _depth := 0.0
var _xf := Transform3D.IDENTITY


func _init() -> void:
	name = "Transform gizmo"
	set_anchors_preset(Control.PRESET_FULL_RECT)
	mouse_filter = Control.MOUSE_FILTER_STOP
	visible = false


## The selection's centre changed (picked, moved, undone).
func refresh() -> void:
	if _handle == Handle.NONE and canvas:
		_center = canvas.selection_center()
	_follow()
	queue_redraw()


## Drops a drag in progress, putting the strokes back (selection gone, tool switched).
func cancel() -> void:
	if _handle != Handle.NONE:
		_handle = Handle.NONE
		_xf = Transform3D.IDENTITY
		canvas.preview_selection_transform(Transform3D.IDENTITY)


func is_dragging() -> bool:
	return _handle != Handle.NONE


## Screen areas of the handles (touches starting there move the selection instead of
## drawing): the pad, the bar, small squares along the ring and the arcs. Strokes inside the
## ring or between handles stay tappable.
func areas() -> Array[Rect2]:
	var s := ui_scale
	var out: Array[Rect2] = [Rect2(_at, Vector2.ZERO).grow((PAD + 6.0) * s), _bar_rect().grow(8.0 * s)]
	var square := func(angle_deg: float, radius: float, half: float) -> Rect2:
		return Rect2(_at + Vector2.from_angle(deg_to_rad(angle_deg)) * radius * s, Vector2.ZERO).grow(half * s)
	for i in 24:
		out.append(square.call(i * 15.0, RING, 9.0))
	for arc in [YAW_ARC, PITCH_ARC]:
		for a in range(int(arc.x), int(arc.y) + 1, 10):
			out.append(square.call(a, ARC, 10.0))
	return out


func _process(_delta: float) -> void:
	if visible:
		_follow()


func _follow() -> void:
	if camera == null:
		return
	var c := _xf * _center
	if camera.is_position_behind(c):
		return
	var at := camera.unproject_position(c)
	if at.distance_to(_at) > 0.5:
		_at = at
		moved.emit()
		# Only when it moved: an idle gizmo mustn't keep the app drawing (race to idle, §10).
		queue_redraw()


func _has_point(point: Vector2) -> bool:
	return _hit(point) != Handle.NONE


func _hit(p: Vector2) -> Handle:
	var s := ui_scale
	var d := p.distance_to(_at)
	if d <= (PAD + 6.0) * s:
		return Handle.MOVE
	if _bar_rect().grow(8.0 * s).has_point(p):
		return Handle.DEPTH
	var angle := rad_to_deg((p - _at).angle())
	if absf(d - ARC * s) <= 12.0 * s:
		if angle >= YAW_ARC.x - 6.0 and angle <= YAW_ARC.y + 6.0:
			return Handle.YAW
		var a := fposmod(angle, 360.0)
		if a >= PITCH_ARC.x - 6.0 and a <= PITCH_ARC.y + 6.0:
			return Handle.PITCH
	if absf(d - RING * s) <= 10.0 * s:
		return Handle.ROLL
	return Handle.NONE


func _bar_rect() -> Rect2:
	var s := ui_scale
	return Rect2(_at + Vector2(ARC + 14.0, -36.0) * s, Vector2(12.0, 72.0) * s)


## Mouse, and touch or Pencil on iPad (touches don't become mouse events in AirDraw).
func _gui_input(event: InputEvent) -> void:
	if event is InputEventMouseButton and event.button_index == MOUSE_BUTTON_LEFT:
		if _press(event.position, event.pressed):
			accept_event()
	elif event is InputEventScreenTouch and event.index == 0:
		if _press(event.position, event.pressed):
			accept_event()
	elif event is InputEventMouseMotion:
		if _handle == Handle.NONE:
			var h := _hit(event.position)
			if h != _hover:
				_hover = h
				queue_redraw()
			return
		_drag(event.position, event.shift_pressed)
		accept_event()
	elif event is InputEventScreenDrag and event.index == 0 and _handle != Handle.NONE:
		_drag(event.position, Input.is_key_pressed(KEY_SHIFT))
		accept_event()


## Starts or ends a drag; true when the event was the gizmo's.
func _press(at: Vector2, pressed: bool) -> bool:
	if pressed:
		_handle = _hit(at)
		if _handle == Handle.NONE:
			return false
		_start = at
		_start_center = _at
		_depth = -(camera.global_transform.affine_inverse() * _center).z
		_xf = Transform3D.IDENTITY
		queue_redraw()
		return true
	if _handle == Handle.NONE:
		return false
	var xf := _xf
	_handle = Handle.NONE
	_xf = Transform3D.IDENTITY
	canvas.commit_selection_transform(xf)
	refresh()
	queue_redraw()
	return true


func _drag(at: Vector2, snap: bool) -> void:
	_xf = _transform_for(at, snap)
	canvas.preview_selection_transform(_xf)
	_follow()


## The transform a drag from the press to `p` asks for.
func _transform_for(p: Vector2, snap: bool) -> Transform3D:
	var basis := camera.global_transform.basis
	match _handle:
		Handle.MOVE:
			var off := camera.project_position(_start_center + (p - _start), _depth) - camera.project_position(_start_center, _depth)
			return Transform3D(Basis.IDENTITY, off)
		Handle.DEPTH:
			# World size of a pixel at the selection's depth; dragging up pushes away.
			var px := (camera.project_position(_start_center + Vector2(0, 1), _depth) - camera.project_position(_start_center, _depth)).length()
			return Transform3D(Basis.IDENTITY, -basis.z * (_start.y - p.y) * px)
		Handle.ROLL:
			# Screen angles grow clockwise (y down); a turn about the axis towards the viewer
			# by a positive angle looks counter-clockwise.
			var turn := wrapf((p - _start_center).angle() - (_start - _start_center).angle(), -PI, PI)
			# About the line of sight to the selection (not the camera's axis), so it turns
			# flat on screen around the gizmo wherever it is in the view.
			var towards_eye := basis.z
			if camera.projection == Camera3D.PROJECTION_PERSPECTIVE:
				towards_eye = (camera.global_position - _center).normalized()
			return _about(towards_eye, -turn, snap)
		Handle.YAW:
			return _about(Vector3.UP, (p.x - _start.x) * TURN_PER_PX, snap)
		Handle.PITCH:
			return _about(basis.x, (p.y - _start.y) * TURN_PER_PX, snap)
	return Transform3D.IDENTITY


## A turn by `angle` about `axis` through the selection's centre.
func _about(axis: Vector3, angle: float, snap: bool) -> Transform3D:
	if snap:
		angle = snappedf(angle, SNAP)
	var b := Basis(axis.normalized(), angle)
	return Transform3D(b, _center - b * _center)


func _draw() -> void:
	var s := ui_scale
	var accent := UiTheme.ACCENT
	var halo := Color(1, 1, 1, 0.85)
	var lit := func(h: Handle) -> Color:
		return accent.lightened(0.35) if h == _handle or (_handle == Handle.NONE and h == _hover) else accent
	draw_arc(_at, RING * s, 0.0, TAU, 72, halo, 5.0 * s, true)
	draw_arc(_at, RING * s, 0.0, TAU, 72, lit.call(Handle.ROLL), 2.5 * s, true)
	for h in [[Handle.YAW, YAW_ARC], [Handle.PITCH, PITCH_ARC]]:
		var from := deg_to_rad(h[1].x)
		var to := deg_to_rad(h[1].y)
		draw_arc(_at, ARC * s, from, to, 24, halo, 9.0 * s, true)
		draw_arc(_at, ARC * s, from, to, 24, lit.call(h[0]), 6.0 * s, true)
	draw_circle(_at, (PAD + 2.0) * s, halo)
	draw_circle(_at, PAD * s, lit.call(Handle.MOVE))
	for d in [Vector2.RIGHT, Vector2.LEFT, Vector2.UP, Vector2.DOWN]:
		draw_line(_at + d * 4.0 * s, _at + d * 11.0 * s, Color.WHITE, 2.0 * s, true)
	var bar := _bar_rect()
	draw_rect(bar.grow(2.0 * s), halo)
	draw_rect(bar, lit.call(Handle.DEPTH))
	var mid := bar.get_center()
	draw_line(mid + Vector2(0, -26) * s, mid + Vector2(0, 26) * s, Color.WHITE, 1.5 * s, true)
