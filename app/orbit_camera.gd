class_name OrbitCamera
extends Node3D
## Orbit/pan/zoom rig. Driven by SketchSession's view signals, so mouse, trackpad,
## touch and pen all reach it through the same input router.

@export var distance := 3.0
@export var orbit_speed := 0.006
@export var min_distance := 0.2
@export var max_distance := 50.0

const DEFAULT_DISTANCE := 3.0
const DEFAULT_YAW := 0.6
const DEFAULT_PITCH := -0.45
## Fit View leaves this much room around the sketch.
const FIT_MARGIN := 1.05
## Fit View ignores anything beyond this from the origin (stray points in old sketches).
const FIT_LIMIT := 200.0

var yaw := DEFAULT_YAW
var pitch := DEFAULT_PITCH
var camera: Camera3D


func _ready() -> void:
	camera = Camera3D.new()
	camera.near = 0.01
	add_child(camera)
	_apply()


## Settles the pivot `depth` ahead of the camera without moving the camera, so the picture
## doesn't change but pan speed, orbit and zoom work at the depth of what a gesture is about.
## With nothing to go by (`depth` < 0), keeps the pivot unless it's too close to be useful.
func focus(depth: float) -> void:
	var d := clampf(depth if depth > 0.0 else maxf(distance, DEFAULT_DISTANCE), min_distance, max_distance)
	var eye := camera.global_position
	global_position = eye - camera.global_transform.basis.z * d
	distance = d
	_apply()


## Frames `box` (all the strokes) at the current angle; an empty box goes back to the start.
func fit(box: AABB, animate := true) -> void:
	box = box.intersection(AABB(Vector3.ONE * -FIT_LIMIT, Vector3.ONE * FIT_LIMIT * 2.0))
	var target_pos := Vector3.ZERO
	var target_distance := DEFAULT_DISTANCE
	if box.size != Vector3.ZERO:
		var half_v := deg_to_rad(camera.fov) * 0.5
		var view := get_viewport().get_visible_rect().size
		var half_h := atan(tan(half_v) * view.x / maxf(view.y, 1.0))
		target_pos = box.get_center()
		target_distance = box.size.length() * 0.5 / sin(minf(half_v, half_h)) * FIT_MARGIN
		target_distance = clampf(target_distance, min_distance, max_distance)
	if not animate:
		global_position = target_pos
		distance = target_distance
		_apply()
		return
	var from_pos := global_position
	var from_distance := distance
	var tween := create_tween().set_trans(Tween.TRANS_CUBIC).set_ease(Tween.EASE_OUT)
	tween.tween_method(func(f: float) -> void:
		global_position = from_pos.lerp(target_pos, f)
		distance = lerpf(from_distance, target_distance, f)
		_apply(), 0.0, 1.0, 0.3)


## The view a sketch opens at: the default angle, framing `box`.
func reset_to(box: AABB) -> void:
	yaw = DEFAULT_YAW
	pitch = DEFAULT_PITCH
	fit(box, false)


## Screen-space drag in pixels.
func orbit(delta: Vector2) -> void:
	yaw -= delta.x * orbit_speed
	pitch = clampf(pitch - delta.y * orbit_speed, -1.55, 1.55)
	_apply()


## Screen-space drag in pixels; the scene follows the pointer.
func pan(delta: Vector2) -> void:
	var scale := distance * 0.0015
	var b := camera.global_transform.basis
	global_position += (-b.x * delta.x + b.y * delta.y) * scale


## Multiplies the distance by `factor` (< 1 zooms in), toward `at` (viewport pixels: the
## pinch centre or the mouse): the point under it stays under it, so you zoom into what you
## point at, wherever it is. Zooming in past `min_distance` moves the whole rig forward
## instead of stopping, so you can always get closer (the pivot used to be a hard wall).
func zoom(factor: float, at := Vector2(-1, -1)) -> void:
	if factor <= 0.0 or not is_finite(factor):
		return
	var target := clampf(distance * factor, min_distance, max_distance)
	var forward := -camera.global_transform.basis.z
	if at.x >= 0.0 and at.y >= 0.0:
		# Where `at` meets the plane through the pivot facing the camera; moving the pivot
		# toward it by the same factor as the distance keeps it fixed on screen.
		var origin := camera.project_ray_origin(at)
		var dir := camera.project_ray_normal(at)
		var denom := dir.dot(forward)
		if denom > 1e-4:
			var anchor := origin + dir * ((global_position - origin).dot(forward) / denom)
			global_position = anchor + (global_position - anchor) * (target / distance)
	if distance * factor < min_distance:
		# At the limit: move forward by the rest of the zoom instead of stopping.
		global_position += forward * (min_distance - distance * factor)
	distance = target
	_apply()


## Animates to a standard view: "front", "back", "side" (right), "left", "top", "bottom" or "3d".
func set_view(view: String) -> void:
	var target: Vector2 = {
		"front": Vector2(0, 0), "back": Vector2(PI, 0), "side": Vector2(PI / 2, 0), "left": Vector2(-PI / 2, 0),
		"top": Vector2(0, -1.55), "bottom": Vector2(0, 1.55), "3d": Vector2(DEFAULT_YAW, DEFAULT_PITCH),
	}.get(view, Vector2(yaw, pitch))
	# Turn the short way round.
	target.x = yaw + wrapf(target.x - yaw, -PI, PI)
	var tween := create_tween().set_trans(Tween.TRANS_CUBIC).set_ease(Tween.EASE_OUT)
	tween.tween_method(func(v: Vector2) -> void:
		yaw = v.x
		pitch = v.y
		_apply(), Vector2(yaw, pitch), target, 0.3)


func _apply() -> void:
	rotation = Vector3(pitch, yaw, 0.0)
	if camera:
		camera.position = Vector3(0, 0, distance)
