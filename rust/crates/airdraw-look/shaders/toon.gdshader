// Toon / cel shading: light falls into two flat bands with a hard step between them, a
// small hard-edged highlight, and a rim light on the lit side of the silhouette. Shadows
// arrive through ATTENUATION, so they are hard-edged too.
shader_type spatial;
render_mode cull_back, diffuse_burley, specular_disabled;

uniform vec4 albedo : source_color = vec4(1.0);
// Brightness of the darkest band, relative to full light.
uniform float shadow_tone : hint_range(0.0, 1.0) = 0.45;
// Where the light/shadow step falls (N·L).
uniform float threshold : hint_range(-1.0, 1.0) = 0.1;
uniform float highlight : hint_range(0.0, 1.0) = 0.35;
uniform float highlight_size : hint_range(0.0, 1.0) = 0.06;
uniform float rim : hint_range(0.0, 1.0) = 0.25;

void fragment() {
	ALBEDO = albedo.rgb;
	ROUGHNESS = 1.0;
}

void light() {
	float ndl = dot(NORMAL, LIGHT) * ATTENUATION;
	// Two bands with a narrow soft edge (2% of N·L) so the step is crisp but not aliased.
	float band = mix(shadow_tone, 1.0, smoothstep(threshold, threshold + 0.02, ndl));
	DIFFUSE_LIGHT += band * LIGHT_COLOR / PI;
	float spec = dot(reflect(-LIGHT, NORMAL), VIEW);
	SPECULAR_LIGHT += smoothstep(1.0 - highlight_size, 1.0 - highlight_size + 0.01, spec) * highlight * ATTENUATION * LIGHT_COLOR;
	float edge = 1.0 - dot(NORMAL, VIEW);
	DIFFUSE_LIGHT += smoothstep(0.62, 0.64, edge) * step(threshold, ndl) * rim * LIGHT_COLOR / PI;
}
