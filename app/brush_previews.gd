class_name BrushPreviews
extends Node
## Renders each brush's sample stroke once, offscreen, into a texture for the brush list
## (light strokes on the dark tray). Cached per brush definition; one render per frame.

signal preview_ready(id: String, texture: Texture2D)

## About the proportions of the preview slot in a brush row, so it isn't scaled down much.
const SIZE := Vector2i(300, 112)

var canvas: SketchCanvas
var _viewport: SubViewport
var _mesh: MeshInstance3D
var _material: StandardMaterial3D
var _cache := {}  ## id → [definition string, texture]
var _queue: Array[String] = []
var _busy := false


func setup(p_canvas: SketchCanvas) -> void:
	canvas = p_canvas
	_viewport = SubViewport.new()
	_viewport.size = SIZE
	_viewport.transparent_bg = true
	_viewport.own_world_3d = true
	_viewport.msaa_3d = Viewport.MSAA_4X
	_viewport.render_target_update_mode = SubViewport.UPDATE_DISABLED
	add_child(_viewport)
	var cam := Camera3D.new()
	cam.projection = Camera3D.PROJECTION_ORTHOGONAL
	cam.size = 0.31
	cam.position = Vector3(0, 0, 2)
	_viewport.add_child(cam)
	_mesh = MeshInstance3D.new()
	_viewport.add_child(_mesh)
	_material = StandardMaterial3D.new()
	_material.shading_mode = BaseMaterial3D.SHADING_MODE_UNSHADED
	_material.albedo_color = Color(0.94, 0.94, 0.96)
	_material.cull_mode = BaseMaterial3D.CULL_DISABLED
	canvas.brushes_changed.connect(func() -> void:
		for id in canvas.brush_ids():
			request(id))


## The cached preview for a brush, or null (then one is queued; `preview_ready` follows).
func get_preview(id: String) -> Texture2D:
	var key := _key(id)
	if _cache.has(id) and _cache[id][0] == key:
		return _cache[id][1]
	request(id)
	return null


func request(id: String) -> void:
	if id not in _queue:
		_queue.append(id)


func _process(_delta: float) -> void:
	if _busy or _queue.is_empty() or canvas == null:
		return
	_busy = true
	var id: String = _queue.pop_front()
	var key := _key(id)
	if _cache.has(id) and _cache[id][0] == key:
		_busy = false
		return
	var result: Dictionary = canvas.brush_preview(canvas.brush_definition(id))
	if not result.error.is_empty():
		_busy = false
		return
	_mesh.mesh = result.mesh
	_mesh.material_override = _material
	_viewport.render_target_update_mode = SubViewport.UPDATE_ONCE
	await RenderingServer.frame_post_draw
	var tex := ImageTexture.create_from_image(_viewport.get_texture().get_image())
	_cache[id] = [key, tex]
	_busy = false
	preview_ready.emit(id, tex)


func _key(id: String) -> String:
	return str(canvas.brush_definition(id))
