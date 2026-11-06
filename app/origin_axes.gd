class_name OriginAxes
extends Node3D
## The sketch's origin and axes (docs/ARCHITECTURE.md §3k): X red, Y green, Z blue lines
## through the origin, solid on the positive side and faint on the negative, fading out with
## distance; X/Y/Z labels a metre out and a dot at the origin, both a fixed size on screen.
## Not strokes: never picked, never in thumbnails or exports.

## How far the lines reach each way (metres).
const LENGTH := 20.0
const COLORS := [Color(0.9, 0.25, 0.3), Color(0.35, 0.75, 0.25), Color(0.25, 0.45, 0.95)]
const AXES := [Vector3.RIGHT, Vector3.UP, Vector3.BACK]
const NAMES := ["X", "Y", "Z"]
## Line width in pixels at a 900 px tall view (scaled with the view, like the UI).
const WIDTH := 2.5
## Lines a fixed width on screen at any distance: each segment is a quad whose corners are
## pushed sideways in clip space. CUSTOM0 holds the segment's other end, CUSTOM1.x the side.
const LINE_SHADER := """
shader_type spatial;
render_mode unshaded, cull_disabled, depth_draw_never, blend_mix, shadows_disabled;
uniform float width_px = 2.5;
void vertex() {
	vec4 a = PROJECTION_MATRIX * MODELVIEW_MATRIX * vec4(VERTEX, 1.0);
	vec4 b = PROJECTION_MATRIX * MODELVIEW_MATRIX * vec4(CUSTOM0.xyz, 1.0);
	vec2 sa = a.xy / max(a.w, 1e-4) * VIEWPORT_SIZE * 0.5;
	vec2 sb = b.xy / max(b.w, 1e-4) * VIEWPORT_SIZE * 0.5;
	vec2 dir = normalize(sb - sa + vec2(1e-5, 0.0));
	vec2 side = vec2(-dir.y, dir.x) * CUSTOM1.x * width_px * VIEWPORT_SIZE.y / 900.0 * 0.5;
	a.xy += side / (VIEWPORT_SIZE * 0.5) * a.w;
	POSITION = a;
}
void fragment() {
	ALBEDO = COLOR.rgb;
	ALPHA = COLOR.a;
}
"""

var _lines: MeshInstance3D
var _labels: Array[Label3D] = []
var _dot: Label3D


func _init() -> void:
	name = "Origin axes"
	_lines = MeshInstance3D.new()
	_lines.cast_shadow = GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	var shader := Shader.new()
	shader.code = LINE_SHADER
	var mat := ShaderMaterial.new()
	mat.shader = shader
	mat.set_shader_parameter("width_px", WIDTH)
	_lines.material_override = mat
	# The quads are pushed out on screen: don't let culling drop them by their thin bounds.
	_lines.extra_cull_margin = 1.0
	_lines.mesh = _build_lines()
	add_child(_lines)
	for i in 3:
		var label := _marker(NAMES[i], COLORS[i], 40)
		label.position = AXES[i] * 1.0
		_labels.append(label)
		add_child(label)
	_dot = _marker("●", Color(0.15, 0.15, 0.18), 30)
	add_child(_dot)


## Places the origin.
func set_origin(at: Vector3) -> void:
	position = at


## Keeps the dot readable on light and dark backgrounds.
func set_background(color: Color) -> void:
	_dot.modulate = Color(0.92, 0.92, 0.95) if color.get_luminance() < 0.45 else Color(0.15, 0.15, 0.18)


func _build_lines() -> ArrayMesh:
	var verts := PackedVector3Array()
	var others := PackedFloat32Array()
	var sides := PackedFloat32Array()
	var colors := PackedColorArray()
	var indices := PackedInt32Array()
	# Each half in segments, so the fade is smooth: solid near the origin, gone at LENGTH.
	const STEPS := 16
	for i in 3:
		for sign in [1.0, -1.0]:
			var base: Color = COLORS[i]
			var a0 := 0.95 if sign > 0.0 else 0.4
			for k in STEPS:
				var t0 := float(k) / STEPS
				var t1 := float(k + 1) / STEPS
				var p0: Vector3 = AXES[i] * sign * LENGTH * t0 * t0
				var p1: Vector3 = AXES[i] * sign * LENGTH * t1 * t1
				var c0 := Color(base, a0 * (1.0 - t0))
				var c1 := Color(base, a0 * (1.0 - t1))
				var n := verts.size()
				# Corners: p0 and p1, each on both sides; each knows the other end.
				for corner in [[p0, p1, 1.0, c0], [p0, p1, -1.0, c0], [p1, p0, -1.0, c1], [p1, p0, 1.0, c1]]:
					verts.append(corner[0])
					var other: Vector3 = corner[1]
					others.append_array([other.x, other.y, other.z, 0.0])
					sides.append_array([corner[2], 0.0, 0.0, 0.0])
					colors.append(corner[3])
				indices.append_array([n, n + 1, n + 2, n, n + 2, n + 3])
	var arrays := []
	arrays.resize(Mesh.ARRAY_MAX)
	arrays[Mesh.ARRAY_VERTEX] = verts
	arrays[Mesh.ARRAY_COLOR] = colors
	arrays[Mesh.ARRAY_CUSTOM0] = others
	arrays[Mesh.ARRAY_CUSTOM1] = sides
	arrays[Mesh.ARRAY_INDEX] = indices
	var fmt := (Mesh.ARRAY_CUSTOM_RGBA_FLOAT << Mesh.ARRAY_FORMAT_CUSTOM0_SHIFT) | (Mesh.ARRAY_CUSTOM_RGBA_FLOAT << Mesh.ARRAY_FORMAT_CUSTOM1_SHIFT)
	var mesh := ArrayMesh.new()
	mesh.add_surface_from_arrays(Mesh.PRIMITIVE_TRIANGLES, arrays, [], {}, fmt)
	return mesh


func _marker(text: String, color: Color, font_size: int) -> Label3D:
	var l := Label3D.new()
	l.text = text
	l.modulate = color
	l.outline_modulate = Color(1, 1, 1, 0.6)
	l.outline_size = 6
	l.font = UiTheme.font(700)
	l.font_size = font_size
	l.fixed_size = true
	l.pixel_size = 0.001
	l.billboard = BaseMaterial3D.BILLBOARD_ENABLED
	l.no_depth_test = true
	l.render_priority = 2
	return l
