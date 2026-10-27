//! Looks: how strokes are shaded, as data (docs/ARCHITECTURE.md §3g). A Look names a shader
//! (its own lighting model), parameters for it, and a lighting environment. The brush says
//! *what* a stroke is (colour, roughness, emission); the Look says *how* it's drawn.
//!
//! Built-in Looks and their shaders are embedded, so they work without file access on every
//! platform. Shaders are Godot spatial shaders with exactly one `render_mode` line starting
//! with `cull_back`; double-sided brushes get a `cull_disabled` variant.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Newest Look file schema this build understands.
pub const SCHEMA_VERSION: u32 = 1;
/// The Look new and older sketches use.
pub const DEFAULT_LOOK: &str = "com.airdraw.shaded";

const BUILTIN_LOOKS: &[(&str, &str)] = &[
    ("shaded", include_str!("../looks/shaded.look.toml")),
    ("flat", include_str!("../looks/flat.look.toml")),
    ("clay", include_str!("../looks/clay.look.toml")),
    ("toon", include_str!("../looks/toon.look.toml")),
    ("ink", include_str!("../looks/ink.look.toml")),
];

const SHADERS: &[(&str, &str)] = &[
    ("lit", include_str!("../shaders/lit.gdshader")),
    ("flat", include_str!("../shaders/flat.gdshader")),
    ("clay", include_str!("../shaders/clay.gdshader")),
    ("toon", include_str!("../shaders/toon.gdshader")),
    ("fill", include_str!("../shaders/fill.gdshader")),
    ("outline", include_str!("../shaders/outline.gdshader")),
];

/// The shader outline passes use.
pub const OUTLINE_SHADER: &str = "outline";

/// Contents of a `.look.toml` file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookDef {
    pub schema: u32,
    pub id: String,
    pub name: String,
    /// Shader for lit brushes.
    pub shader: String,
    /// Shader for brushes whose material is unlit; defaults to `shader`.
    #[serde(default)]
    pub unshaded_shader: Option<String>,
    /// Uniform values for the shader, by name.
    #[serde(default)]
    pub params: BTreeMap<String, f32>,
    /// A second pass that draws each stroke's outline (toon, line art).
    #[serde(default)]
    pub outline: Option<Outline>,
    pub environment: Environment,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outline {
    /// sRGB hex.
    pub color: String,
    /// Pixels at a 900 px tall view; scales with the view.
    pub width: f32,
}

/// The lighting around the strokes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    /// sRGB hex.
    pub background: String,
    /// Ambient light colour, sRGB hex.
    pub ambient: String,
    pub ambient_energy: f32,
    /// Key light strength; 0 turns it off.
    pub sun_energy: f32,
    pub shadows: bool,
    /// Screen-space ambient occlusion where the renderer has it (desktop).
    pub ssao: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LookError {
    Parse(String),
    NewerSchema(u32),
    UnknownShader(String),
    BadColor(String),
}

impl fmt::Display for LookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LookError::Parse(e) => write!(f, "invalid look file: {e}"),
            LookError::NewerSchema(v) => {
                write!(f, "look uses format version {v}; this app supports up to {SCHEMA_VERSION}. Update the app.")
            }
            LookError::UnknownShader(s) => write!(f, "unknown shader '{s}'"),
            LookError::BadColor(c) => write!(f, "'{c}' is not a #RRGGBB colour"),
        }
    }
}

impl std::error::Error for LookError {}

impl LookDef {
    pub fn from_toml(text: &str) -> Result<Self, LookError> {
        let def: Self = toml::from_str(text).map_err(|e| LookError::Parse(e.to_string()))?;
        def.validate()?;
        Ok(def)
    }

    /// Checks a Look this build can render: known schema and shaders, valid colours. For
    /// Looks that didn't come through [`LookDef::from_toml`], e.g. a sketch's embedded copy.
    pub fn validate(&self) -> Result<(), LookError> {
        if self.schema > SCHEMA_VERSION {
            return Err(LookError::NewerSchema(self.schema));
        }
        for s in std::iter::once(&self.shader).chain(&self.unshaded_shader) {
            if shader_source(s).is_none() {
                return Err(LookError::UnknownShader(s.clone()));
            }
        }
        parse_color(&self.environment.background)?;
        parse_color(&self.environment.ambient)?;
        if let Some(o) = &self.outline {
            parse_color(&o.color)?;
        }
        Ok(())
    }

    /// The shader for a brush: its unlit variant for unlit brushes, if the Look has one.
    pub fn shader_for(&self, unshaded_brush: bool) -> &str {
        match (&self.unshaded_shader, unshaded_brush) {
            (Some(s), true) => s,
            _ => &self.shader,
        }
    }
}

/// Built-in Looks, by id, in menu order.
pub struct LookLibrary {
    looks: Vec<LookDef>,
}

impl Default for LookLibrary {
    fn default() -> Self {
        Self::with_builtins()
    }
}

impl LookLibrary {
    pub fn with_builtins() -> Self {
        let looks = BUILTIN_LOOKS
            .iter()
            .map(|(name, text)| LookDef::from_toml(text).unwrap_or_else(|e| panic!("built-in look '{name}': {e}")))
            .collect();
        Self { looks }
    }

    pub fn get(&self, id: &str) -> Option<&LookDef> {
        self.looks.iter().find(|l| l.id == id)
    }

    /// The Look with `id`, or the default one for unknown ids (e.g. from a newer app).
    pub fn get_or_default(&self, id: Option<&str>) -> &LookDef {
        id.and_then(|id| self.get(id)).or_else(|| self.get(DEFAULT_LOOK)).expect("default look is built in")
    }

    pub fn iter(&self) -> impl Iterator<Item = &LookDef> {
        self.looks.iter()
    }
}

fn shader_source(name: &str) -> Option<&'static str> {
    SHADERS.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
}

/// Godot shader code for `name`; double-sided brushes get a variant that culls no faces.
/// (Stroke shaders cull back faces; the outline pass culls front faces: an inverted hull.)
pub fn shader_code(name: &str, double_sided: bool) -> Result<String, LookError> {
    let src = shader_source(name).ok_or_else(|| LookError::UnknownShader(name.into()))?;
    if !double_sided {
        return Ok(src.to_string());
    }
    let culled = ["render_mode cull_back", "render_mode cull_front"];
    let from = culled.iter().find(|c| src.contains(**c)).expect("shaders start render_mode with a cull mode");
    Ok(src.replacen(from, "render_mode cull_disabled", 1))
}

/// `#RRGGBB` → sRGB components 0..1.
pub fn parse_color(hex: &str) -> Result<[f32; 3], LookError> {
    let bad = || LookError::BadColor(hex.into());
    let h = hex.strip_prefix('#').ok_or_else(bad)?;
    if h.len() != 6 {
        return Err(bad());
    }
    let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).map(|v| v as f32 / 255.0).map_err(|_| bad());
    Ok([c(0)?, c(2)?, c(4)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_load_with_unique_ids_and_a_default() {
        let lib = LookLibrary::with_builtins();
        let ids: Vec<&str> = lib.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(ids, ["com.airdraw.shaded", "com.airdraw.flat", "com.airdraw.clay", "com.airdraw.toon", "com.airdraw.ink"]);
        assert!(lib.get("com.airdraw.toon").unwrap().outline.is_some());
        assert!(lib.get("com.airdraw.shaded").unwrap().outline.is_none());
        assert_eq!(lib.get_or_default(None).id, DEFAULT_LOOK);
        assert_eq!(lib.get_or_default(Some("com.someone.future")).id, DEFAULT_LOOK);
    }

    #[test]
    fn every_shader_has_one_cullable_render_mode() {
        for (name, src) in SHADERS {
            assert_eq!(src.matches("render_mode").count(), 1, "{name}");
            let outline = *name == OUTLINE_SHADER;
            assert!(src.contains(if outline { "render_mode cull_front" } else { "render_mode cull_back" }), "{name}");
            let two_sided = shader_code(name, true).unwrap();
            assert!(two_sided.contains("render_mode cull_disabled"), "{name}");
            assert!(!two_sided.contains("cull_back") && !two_sided.contains("cull_front"), "{name}");
            assert_eq!(shader_code(name, false).unwrap(), *src);
            if !outline {
                assert!(src.contains("uniform vec4 albedo"), "{name}: every stroke shader takes the brush colour");
            }
        }
    }

    #[test]
    fn unlit_brushes_use_the_unshaded_shader_when_given() {
        let lib = LookLibrary::with_builtins();
        let shaded = lib.get("com.airdraw.shaded").unwrap();
        assert_eq!((shaded.shader_for(false), shaded.shader_for(true)), ("lit", "flat"));
        let clay = lib.get("com.airdraw.clay").unwrap();
        assert_eq!((clay.shader_for(false), clay.shader_for(true)), ("clay", "clay"));
    }

    #[test]
    fn helpful_errors() {
        let ok = include_str!("../looks/clay.look.toml");
        assert!(matches!(LookDef::from_toml(&ok.replace("\"clay\"\n", "\"crayon\"\n")), Err(LookError::UnknownShader(_))));
        assert!(matches!(LookDef::from_toml(&ok.replace("schema = 1", "schema = 9")), Err(LookError::NewerSchema(9))));
        assert!(matches!(LookDef::from_toml(&ok.replace("#D6D4CF", "grey")), Err(LookError::BadColor(_))));
        assert!(matches!(LookDef::from_toml(&ok.replace("shadows = true", "shadow = true")), Err(LookError::Parse(_))));
        let toon = include_str!("../looks/toon.look.toml");
        assert!(matches!(LookDef::from_toml(&toon.replace("#16161C", "black")), Err(LookError::BadColor(_))));
    }

    #[test]
    fn colors_parse() {
        assert_eq!(parse_color("#FF0080").unwrap(), [1.0, 0.0, 128.0 / 255.0]);
        assert!(parse_color("FF0080").is_err() && parse_color("#FF00").is_err() && parse_color("#GG0000").is_err());
    }
}
