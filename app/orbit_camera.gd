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


## Multiplies the distance by `factor` (< 1 zooms in).
func zoom(factor: float, _at := Vector2.ZERO) -> void:
	distance = clampf(distance * factor, min_distance, max_distance)
	_apply()


## Animates to a standard view: "front", "side", "top" or "3d".
func set_view(view: String) -> void:
	var target: Vector2 = {"front": Vector2(0, 0), "side": Vector2(PI / 2, 0), "top": Vector2(0, -1.55), "3d": Vector2(0.6, -0.45)}.get(view, Vector2(yaw, pitch))
	var tween := create_tween().set_trans(Tween.TRANS_CUBIC).set_ease(Tween.EASE_OUT)
	tween.tween_method(func(v: Vector2) -> void:
		yaw = v.x
		pitch = v.y
		_apply(), Vector2(yaw, pitch), target, 0.3)


func _apply() -> void:
	rotation = Vector3(pitch, yaw, 0.0)
	if camera:
		camera.position = Vector3(0, 0, distance)
