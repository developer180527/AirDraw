class_name ThumbnailRenderer
extends SubViewport
## Renders a sketch preview offscreen for the file's embedded thumbnail (OS thumbnailers,
## Recents). Shares the canvas's 3D world but sees only the stroke render layer, so grids
## and guides never appear, and frames all strokes from one fixed angle (VIEW), never the
## user's camera: a file must not change because someone looked at it differently (§3h).

const SIZE := 512
## Must match STROKE_LAYERS in the Rust canvas (layer 20).
const STROKE_LAYER := 20
## Front, a little from the left and above: the angle a new sketch opens at (OrbitCamera's
## default yaw 0.6, pitch -0.45).
static var VIEW := Basis.from_euler(Vector3(-0.45, 0.6, 0.0))

var _camera: Camera3D


func _init() -> void:
	size = Vector2i(SIZE, SIZE)
	msaa_3d = Viewport.MSAA_4X
	render_target_update_mode = SubViewport.UPDATE_DISABLED
	_camera = Camera3D.new()
	_camera.cull_mask = 1 << (STROKE_LAYER - 1)
	var env := Environment.new()
	env.background_mode = Environment.BG_COLOR
	env.background_color = Color(0.95, 0.95, 0.93)
	env.ambient_light_source = Environment.AMBIENT_SOURCE_COLOR
	env.ambient_light_color = Color(0.75, 0.75, 0.8)
	_camera.environment = env
	add_child(_camera)


## PNG bytes of all strokes seen from VIEW, or empty when there is nothing to draw. Must be
## awaited: rendering takes one frame.
func render(canvas: SketchCanvas, view: Basis = VIEW) -> PackedByteArray:
	if canvas.stroke_count() == 0:
		return PackedByteArray()
	world_3d = canvas.get_world_3d()
	# Orthographic and fitted to the sketch's outline as seen along `view`: tight framing,
	# and no perspective distortion for a small preview.
	var basis := view.orthonormalized()
	var box: AABB = canvas.strokes_aabb()
	var lo := Vector3(INF, INF, INF)
	var hi := -lo
	for i in 8:
		var local := basis.transposed() * box.get_endpoint(i)
		lo = lo.min(local)
		hi = hi.max(local)
	var extent := hi - lo
	_camera.projection = Camera3D.PROJECTION_ORTHOGONAL
	_camera.size = maxf(maxf(extent.x, extent.y) * 1.1, 0.01)
	var mid := (lo + hi) * 0.5
	# Stand back along the view axis, beyond the nearest point.
	var eye_local := Vector3(mid.x, mid.y, hi.z + 1.0)
	_camera.near = 0.01
	_camera.far = extent.z + 2.0
	_camera.global_transform = Transform3D(basis, basis * eye_local)
	render_target_update_mode = SubViewport.UPDATE_ONCE
	await RenderingServer.frame_post_draw
	return get_texture().get_image().save_png_to_buffer()
