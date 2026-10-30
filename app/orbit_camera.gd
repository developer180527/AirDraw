class_name OrbitCamera
extends Node3D
## Orbit/pan/zoom rig. Driven by SketchSession's view signals, so mouse, trackpad,
## touch and pen all reach it through the same input router.

@export var distance := 3.0
@export var orbit_speed := 0.006
@export var min_distance := 0.2
@export var max_distance := 50.0

var yaw := 0.6
var pitch := -0.45
var camera: Camera3D


func _ready() -> void:
	camera = Camera3D.new()
	camera.near = 0.01
	add_child(camera)
	_apply()


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
		"top": Vector2(0, -1.55), "bottom": Vector2(0, 1.55), "3d": Vector2(0.6, -0.45),
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
