// A flat fill colour regardless of the brush (paper white for line art); the outline pass
// does the drawing.
shader_type spatial;
render_mode cull_back, unshaded;

uniform vec4 albedo : source_color = vec4(1.0);
uniform vec4 fill_color : source_color = vec4(1.0);

void fragment() {
	ALBEDO = fill_color.rgb;
}
