// Physically based shading (Godot's default lighting), matching StandardMaterial3D:
// Burley diffuse, GGX specular. Brush colour, roughness, metallic and emission come in as
// uniforms. Double-sided brushes get a variant that draws back faces too.
shader_type spatial;
render_mode cull_back, diffuse_burley, specular_schlick_ggx;

uniform vec4 albedo : source_color = vec4(1.0);
uniform float roughness : hint_range(0.0, 1.0) = 0.6;
uniform float metallic : hint_range(0.0, 1.0) = 0.0;
uniform float emission_energy = 0.0;

void fragment() {
	ALBEDO = albedo.rgb;
	ROUGHNESS = roughness;
	METALLIC = metallic;
	EMISSION = albedo.rgb * emission_energy;
}
