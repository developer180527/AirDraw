//! Brush definitions (the TOML file format) and compiled brushes ready to build meshes.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::params::{ParamSpec, Params};
use crate::registry::StageRegistry;
use crate::stage::{Dynamics, Filter, Geometry, InputPoint, MaterialDesc, MeshBuffers, StrokePoint};
use crate::stream::BrushStream;

/// Newest brush file schema this build understands.
pub const SCHEMA_VERSION: u32 = 1;

/// A stage reference in a brush file: `type = "..."` plus that stage's parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StageRef {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(flatten)]
    pub params: Params,
}

impl StageRef {
    pub fn new(kind: &str) -> Self {
        Self { kind: kind.into(), params: Params::new() }
    }
}

/// Contents of a `.brush.toml` file. Built-in and user brushes share this format.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrushDef {
    pub schema: u32,
    /// Reverse-DNS style, e.g. `com.airdraw.ink`. User brushes override built-ins with the same id.
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default)]
    pub filters: Vec<StageRef>,
    #[serde(default)]
    pub dynamics: Vec<StageRef>,
    pub geometry: StageRef,
    pub material: StageRef,
}

impl BrushDef {
    pub fn from_toml(text: &str) -> Result<Self, BrushError> {
        let def: Self = toml::from_str(text).map_err(|e| BrushError::Parse(e.to_string()))?;
        if def.schema > SCHEMA_VERSION {
            return Err(BrushError::NewerSchema(def.schema));
        }
        // Future schema migrations go here, upgrading `def` step by step to SCHEMA_VERSION.
        Ok(def)
    }

    pub fn to_toml(&self) -> Result<String, BrushError> {
        toml::to_string(self).map_err(|e| BrushError::Parse(e.to_string()))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum BrushError {
    Parse(String),
    NewerSchema(u32),
    UnknownStage { slot: &'static str, kind: String },
    UnknownParam { stage: String, name: String },
    BadParam { stage: String, name: String, reason: String },
    Io(String),
    /// The operation isn't allowed, e.g. deleting a built-in brush.
    NotAllowed(String),
}

impl fmt::Display for BrushError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BrushError::Parse(e) => write!(f, "invalid brush file: {e}"),
            BrushError::NewerSchema(v) => {
                write!(f, "brush uses format version {v}; this app supports up to {SCHEMA_VERSION}. Update the app.")
            }
            BrushError::UnknownStage { slot, kind } => write!(f, "unknown {slot} stage '{kind}'"),
            BrushError::UnknownParam { stage, name } => write!(f, "stage '{stage}' has no parameter '{name}'"),
            BrushError::BadParam { stage, name, reason } => write!(f, "'{stage}.{name}': {reason}"),
            BrushError::Io(e) | BrushError::NotAllowed(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for BrushError {}

/// A validated brush: stage implementations paired with complete parameter sets.
pub struct Brush {
    pub def: BrushDef,
    filters: Vec<(Arc<dyn Filter>, Params)>,
    dynamics: Vec<(Arc<dyn Dynamics>, Params)>,
    geometry: (Arc<dyn Geometry>, Params),
    material: MaterialDesc,
}

impl Brush {
    /// Resolves stage names and checks every parameter against its stage's schema.
    pub fn compile(def: BrushDef, reg: &StageRegistry) -> Result<Self, BrushError> {
        fn find<T: ?Sized>(
            map: &std::collections::BTreeMap<String, Arc<T>>,
            slot: &'static str,
            r: &StageRef,
        ) -> Result<Arc<T>, BrushError> {
            map.get(&r.kind).cloned().ok_or_else(|| BrushError::UnknownStage { slot, kind: r.kind.clone() })
        }
        let filters = def
            .filters
            .iter()
            .map(|r| {
                let s = find(&reg.filters, "filter", r)?;
                let p = resolve(r, &s.schema())?;
                Ok((s, p))
            })
            .collect::<Result<Vec<_>, BrushError>>()?;
        let dynamics = def
            .dynamics
            .iter()
            .map(|r| {
                let s = find(&reg.dynamics, "dynamics", r)?;
                let p = resolve(r, &s.schema())?;
                Ok((s, p))
            })
            .collect::<Result<Vec<_>, BrushError>>()?;
        let g = find(&reg.geometry, "geometry", &def.geometry)?;
        let gp = resolve(&def.geometry, &g.schema())?;
        let m = find(&reg.materials, "material", &def.material)?;
        let material = m.describe(&resolve(&def.material, &m.schema())?);
        Ok(Self { def, filters, dynamics, geometry: (g, gp), material })
    }

    pub fn id(&self) -> &str {
        &self.def.id
    }

    pub fn material(&self) -> &MaterialDesc {
        &self.material
    }

    /// Runs the full pipeline. Pure function of the input, so strokes can be rebuilt from
    /// their raw points at any time (re-brushing, export quality, replay). Goes through
    /// [`stream`](Self::stream), so a stroke drawn live ends exactly as it rebuilds.
    pub fn build(&self, input: &[InputPoint]) -> MeshBuffers {
        let mut s = self.stream();
        s.push(input);
        s.mesh(&[])
    }

    /// The processed stroke path (after filters and dynamics) with per-point width.
    /// Exporters use it to write strokes as curves.
    pub fn centerline(&self, input: &[InputPoint]) -> Vec<StrokePoint> {
        let mut s = self.stream();
        s.push(input);
        s.centerline(&[])
    }

    /// Starts an incremental build, for a stroke that grows while it's drawn.
    pub fn stream(&self) -> BrushStream {
        BrushStream::new(&self.filters, &self.dynamics, &self.geometry)
    }
}

/// Schema defaults overlaid with validated values from the file.
pub fn resolve(r: &StageRef, schema: &[ParamSpec]) -> Result<Params, BrushError> {
    let mut out: Params = schema.iter().map(|s| (s.name.to_string(), s.default.clone())).collect();
    for (name, value) in &r.params {
        let spec = schema
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| BrushError::UnknownParam { stage: r.kind.clone(), name: name.clone() })?;
        let v = spec
            .validate(value)
            .map_err(|reason| BrushError::BadParam { stage: r.kind.clone(), name: name.clone(), reason })?;
        out.insert(name.clone(), v);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::ParamValue;
    use glam::Vec3;

    const INK: &str = r##"
schema = 1
id = "test.ink"
name = "Ink"

[[filters]]
type = "one_euro"
min_cutoff = 2

[[dynamics]]
type = "width"
base = 0.02
pressure_curve = [[0.0, 0.5], [1.0, 1.0]]

[geometry]
type = "tube"
sides = 6

[material]
type = "solid"
color = "#FF8800"
"##;

    fn input(n: usize) -> Vec<InputPoint> {
        (0..n)
            .map(|i| InputPoint {
                pos: Vec3::new(i as f32 * 0.01, 0.0, 0.0),
                normal: Vec3::Z,
                pressure: 1.0,
                tilt: None,
                t: i as f64 / 120.0,
            })
            .collect()
    }

    #[test]
    fn parses_compiles_and_builds() {
        let def = BrushDef::from_toml(INK).unwrap();
        assert_eq!(def.filters[0].params.get("min_cutoff"), Some(&ParamValue::Int(2)));
        let brush = Brush::compile(def, &StageRegistry::with_builtins()).unwrap();
        assert_eq!(brush.material().color[0], 1.0);
        let mesh = brush.build(&input(10));
        assert_eq!(mesh.positions.len(), 10 * 6 + 2);
    }

    #[test]
    fn toml_roundtrip() {
        let def = BrushDef::from_toml(INK).unwrap();
        let again = BrushDef::from_toml(&def.to_toml().unwrap()).unwrap();
        assert_eq!(def, again);
    }

    #[test]
    fn helpful_errors() {
        let reg = StageRegistry::with_builtins();
        let compile = |s: &str| Brush::compile(BrushDef::from_toml(s)?, &reg).map(|_| ());
        let e = compile(&INK.replace("\"tube\"", "\"spaghetti\"")).unwrap_err();
        assert_eq!(e, BrushError::UnknownStage { slot: "geometry", kind: "spaghetti".into() });
        let e = compile(&INK.replace("sides = 6", "colour = 6")).unwrap_err();
        assert!(matches!(e, BrushError::UnknownParam { .. }));
        let e = compile(&INK.replace("#FF8800", "orange")).unwrap_err();
        assert!(matches!(e, BrushError::BadParam { .. }), "{e}");
        let e = compile(&INK.replace("schema = 1", "schema = 99")).unwrap_err();
        assert_eq!(e, BrushError::NewerSchema(99));
    }

    #[test]
    fn single_point_builds_nothing() {
        let brush = Brush::compile(BrushDef::from_toml(INK).unwrap(), &StageRegistry::with_builtins()).unwrap();
        assert!(brush.build(&input(1)).is_empty());
    }
}
