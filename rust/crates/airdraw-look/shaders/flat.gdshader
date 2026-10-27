// Unlit: the brush colour as is, like marker on paper.
shader_type spatial;
render_mode cull_back, unshaded;

uniform vec4 albedo : source_color = vec4(1.0);

void fragment() {
	ALBEDO = albedo.rgb;
}
