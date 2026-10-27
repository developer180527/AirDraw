// Clay: a matte clay colour lightly tinted by the brush, soft wrap-around diffuse, no
// specular, and an occlusion term from the world-space normal (darker facing down). The
// Mobile renderer has no screen-space AO, so this stands in for it everywhere.
shader_type spatial;
render_mode cull_back, diffuse_lambert_wrap, specular_disabled;

uniform vec4 albedo : source_color = vec4(1.0);
uniform vec4 clay_color : source_color = vec4(0.84, 0.82, 0.79, 1.0);
uniform float tint : hint_range(0.0, 1.0) = 0.2;
uniform float occlusion : hint_range(0.0, 1.0) = 0.45;

void fragment() {
	ALBEDO = mix(clay_color.rgb, albedo.rgb, tint);
	ROUGHNESS = 1.0;
	vec3 world_normal = (INV_VIEW_MATRIX * vec4(NORMAL, 0.0)).xyz;
	AO = mix(1.0 - occlusion, 1.0, world_normal.y * 0.5 + 0.5);
	AO_LIGHT_AFFECT = 0.6;
}
