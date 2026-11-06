class_name HsvPicker
extends Control
## Colour picker for touch, Pencil and mouse: a saturation/value square and a hue strip.
## (Godot's ColorPicker only follows the mouse.) Emits `color_changed` while dragging.

signal color_changed(color: Color)

## Width of the hue strip and the gap before it, as fractions of the control's height.
const STRIP := 0.16
const GAP := 0.06
const CELLS := 12

var ui_scale := 1.0
var color: Color:
	set(c):
		color = c
		if c.s > 0.0 and c.v > 0.0:
			_h = c.h
		_s = c.s
		_v = c.v
		queue_redraw()

var _h := 0.0
var _s := 0.0
var _v := 1.0
## What the current drag moves: 0 none, 1 the square, 2 the hue strip.
var _drag := 0


func _init() -> void:
	mouse_filter = Control.MOUSE_FILTER_STOP


func _square() -> Rect2:
	var w := size.x - size.y * (STRIP + GAP)
	return Rect2(Vector2.ZERO, Vector2(maxf(w, 1.0), size.y))


func _strip() -> Rect2:
	var sq := _square()
	return Rect2(Vector2(sq.end.x + size.y * GAP, 0.0), Vector2(size.y * STRIP, size.y))


func _gui_input(event: InputEvent) -> void:
	var pressed := false
	var released := false
	var at := Vector2.ZERO
	if event is InputEventScreenTouch and event.index == 0:
		pressed = event.pressed
		released = not event.pressed
		at = event.position
	elif event is InputEventMouseButton and event.button_index == MOUSE_BUTTON_LEFT:
		pressed = event.pressed
		released = not event.pressed
		at = event.position
	elif (event is InputEventScreenDrag and event.index == 0) or (event is InputEventMouseMotion and _drag != 0):
		at = event.position
	else:
		return
	if pressed:
		_drag = 2 if at.x >= _square().end.x else 1
	if released:
		_drag = 0
		accept_event()
		return
	if _drag == 1:
		var sq := _square()
		_s = clampf(at.x / sq.size.x, 0.0, 1.0)
		_v = clampf(1.0 - at.y / sq.size.y, 0.0, 1.0)
	elif _drag == 2:
		_h = clampf(at.y / size.y, 0.0, 0.9999)
	if _drag != 0:
		color = Color.from_hsv(_h, _s, _v)
		color_changed.emit(color)
	accept_event()


func _draw() -> void:
	var sq := _square()
	# The square in cells, each a quad whose corner colours are exact: smooth enough, and
	# exact at every cell corner.
	for i in CELLS:
		for j in CELLS:
			var s0 := float(i) / CELLS
			var s1 := float(i + 1) / CELLS
			var v0 := 1.0 - float(j) / CELLS
			var v1 := 1.0 - float(j + 1) / CELLS
			var pts := PackedVector2Array([
				sq.position + Vector2(s0, 1.0 - v0) * sq.size, sq.position + Vector2(s1, 1.0 - v0) * sq.size,
				sq.position + Vector2(s1, 1.0 - v1) * sq.size, sq.position + Vector2(s0, 1.0 - v1) * sq.size])
			draw_polygon(pts, PackedColorArray([Color.from_hsv(_h, s0, v0), Color.from_hsv(_h, s1, v0),
				Color.from_hsv(_h, s1, v1), Color.from_hsv(_h, s0, v1)]))
	var st := _strip()
	for k in 12:
		var a := float(k) / 12
		var b := float(k + 1) / 12
		draw_polygon(PackedVector2Array([st.position + Vector2(0, a * st.size.y), st.position + Vector2(st.size.x, a * st.size.y),
			st.position + Vector2(st.size.x, b * st.size.y), st.position + Vector2(0, b * st.size.y)]),
			PackedColorArray([Color.from_hsv(a, 1, 1), Color.from_hsv(a, 1, 1), Color.from_hsv(b, 1, 1), Color.from_hsv(b, 1, 1)]))
	var r := 7.0 * ui_scale
	var cursor := sq.position + Vector2(_s, 1.0 - _v) * sq.size
	draw_arc(cursor, r + ui_scale, 0, TAU, 24, Color(0, 0, 0, 0.5), 1.5 * ui_scale, true)
	draw_arc(cursor, r, 0, TAU, 24, Color.WHITE, 2.0 * ui_scale, true)
	var y := _h * st.size.y
	draw_rect(Rect2(st.position + Vector2(-2 * ui_scale, y - 3 * ui_scale), Vector2(st.size.x + 4 * ui_scale, 6 * ui_scale)), Color.WHITE, false, 2.0 * ui_scale)
