// Outline pass, drawn after the stroke's own material (next_pass). Each vertex is pushed out
// in screen space so the line has the same width at any distance; widths are in pixels at a
// 900 px tall view and scale with the view.
//   Tubes and dots: an inverted hull (front faces culled), pushed out along the normal.
//   Flat ribbons (double-sided brushes, `flat_ribbon` = true): no thickness to invert, so each edge is
//   pushed sideways (across the ribbon, along ±BINORMAL by which edge it is) and the copy is
//   moved slightly behind the ribbon in depth.
shader_type spatial;
render_mode cull_front, unshaded, shadows_disabled;

uniform vec4 outline_color : source_color = vec4(0.08, 0.08, 0.1, 1.0);
uniform float outline_width = 2.5;
uniform bool flat_ribbon = false;

void vertex() {
	vec3 dir = flat_ribbon ? BINORMAL * (UV.y * 2.0 - 1.0) : NORMAL;
	vec4 clip = PROJECTION_MATRIX * (MODELVIEW_MATRIX * vec4(VERTEX, 1.0));
	vec3 view_dir = mat3(VIEW_MATRIX) * (MODEL_NORMAL_MATRIX * dir);
	vec2 screen_dir = (PROJECTION_MATRIX * vec4(view_dir, 0.0)).xy;
	if (length(screen_dir) > 1e-6) {
		float px = outline_width * VIEWPORT_SIZE.y / 900.0;
		clip.xy += normalize(screen_dir) * px * 2.0 / VIEWPORT_SIZE * clip.w;
	}
	if (flat_ribbon) {
		// Reverse-Z: smaller is farther. Just enough to lose to the ribbon itself.
		clip.z -= 0.00002 * clip.w;
	}
	POSITION = clip;
}

void fragment() {
	ALBEDO = outline_color.rgb;
}
