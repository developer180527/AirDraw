use crate::params::{ParamSpec, Params, ParamsExt};
use crate::stage::{Material, MaterialDesc, Shading, Stage};

/// Lit surface with PBR-style controls. Emission > 0 makes it glow in its own color.
pub struct Solid;

impl Stage for Solid {
    fn schema(&self) -> Vec<ParamSpec> {
        vec![
            ParamSpec::color("color", "#1E1E24", "Stroke color"),
            ParamSpec::float("roughness", 0.6, 0.0, 1.0, "0 = glossy, 1 = matte"),
            ParamSpec::float("metallic", 0.0, 0.0, 1.0, ""),
            ParamSpec::float("emission", 0.0, 0.0, 16.0, "Glow strength"),
            ParamSpec::bool("double_sided", false, "Render back faces (needed for ribbons)"),
        ]
    }
}

impl Material for Solid {
    fn describe(&self, p: &Params) -> MaterialDesc {
        MaterialDesc {
            shading: Shading::Lit,
            color: p.color("color"),
            roughness: p.float("roughness"),
            metallic: p.float("metallic"),
            emission: p.float("emission"),
            double_sided: p.flag("double_sided"),
        }
    }
}

/// Flat color, no lighting: reads like ink on paper from every angle.
pub struct Flat;

impl Stage for Flat {
    fn schema(&self) -> Vec<ParamSpec> {
        vec![ParamSpec::color("color", "#1E1E24", "Stroke color"), ParamSpec::bool("double_sided", true, "Render back faces")]
    }
}

impl Material for Flat {
    fn describe(&self, p: &Params) -> MaterialDesc {
        MaterialDesc {
            shading: Shading::Unshaded,
            color: p.color("color"),
            roughness: 1.0,
            metallic: 0.0,
            emission: 0.0,
            double_sided: p.flag("double_sided"),
        }
    }
}
