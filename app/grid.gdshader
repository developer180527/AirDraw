shader_type spatial;
render_mode unshaded, cull_disabled, depth_draw_never, blend_mix;

uniform vec4 line_color : source_color = vec4(0.3, 0.45, 0.95, 0.55);
uniform vec4 fill_color : source_color = vec4(0.3, 0.45, 0.95, 0.06);
uniform float cell = 0.1;
// Drawn guides are curved: their grid follows the mesh UVs (world units) instead of local XZ.
uniform bool use_uv = false;

varying vec3 local_pos;

void vertex() {
	local_pos = VERTEX;
}

void fragment() {
	vec2 c = (use_uv ? UV : local_pos.xz) / cell;
	vec2 g = abs(fract(c - 0.5) - 0.5) / fwidth(c);
	float line = 1.0 - min(min(g.x, g.y), 1.0);
	vec4 col = mix(fill_color, line_color, line);
	ALBEDO = col.rgb;
	ALPHA = col.a;
}
