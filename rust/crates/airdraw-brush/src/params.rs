//! Stage parameters: the values stored in brush files, and the schemas stages declare so
//! brush files can be validated and the brush editor UI can be generated.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Points `[x, y]` with x in 0..=1, ascending; evaluated with linear interpolation.
    Curve(Vec<[f32; 2]>),
    /// Plain text, or a `#RRGGBB` / `#RRGGBBAA` color when the spec's hint is `Color`.
    Str(String),
}

impl ParamValue {
    fn kind_name(&self) -> &'static str {
        match self {
            ParamValue::Bool(_) => "bool",
            ParamValue::Int(_) => "integer",
            ParamValue::Float(_) => "number",
            ParamValue::Curve(_) => "curve",
            ParamValue::Str(_) => "string",
        }
    }
}

pub type Params = BTreeMap<String, ParamValue>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hint {
    None,
    Color,
}

/// Declares one parameter of a stage. The value kind comes from `default`.
#[derive(Clone, Debug)]
pub struct ParamSpec {
    pub name: &'static str,
    pub default: ParamValue,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub hint: Hint,
    pub doc: &'static str,
}

impl ParamSpec {
    pub fn float(name: &'static str, default: f64, min: f64, max: f64, doc: &'static str) -> Self {
        Self { name, default: ParamValue::Float(default), min: Some(min), max: Some(max), hint: Hint::None, doc }
    }
    pub fn int(name: &'static str, default: i64, min: i64, max: i64, doc: &'static str) -> Self {
        Self {
            name,
            default: ParamValue::Int(default),
            min: Some(min as f64),
            max: Some(max as f64),
            hint: Hint::None,
            doc,
        }
    }
    pub fn bool(name: &'static str, default: bool, doc: &'static str) -> Self {
        Self { name, default: ParamValue::Bool(default), min: None, max: None, hint: Hint::None, doc }
    }
    pub fn curve(name: &'static str, default: &[[f32; 2]], doc: &'static str) -> Self {
        Self { name, default: ParamValue::Curve(default.to_vec()), min: None, max: None, hint: Hint::None, doc }
    }
    pub fn color(name: &'static str, default: &str, doc: &'static str) -> Self {
        Self { name, default: ParamValue::Str(default.into()), min: None, max: None, hint: Hint::Color, doc }
    }

    /// Checks `value` against this spec. Numbers are clamped into range; ints are accepted
    /// where floats are expected.
    pub fn validate(&self, value: &ParamValue) -> Result<ParamValue, String> {
        let clamp = |v: f64| {
            let v = self.min.map_or(v, |m| v.max(m));
            self.max.map_or(v, |m| v.min(m))
        };
        match (&self.default, value) {
            (ParamValue::Float(_), ParamValue::Float(v)) => Ok(ParamValue::Float(clamp(*v))),
            (ParamValue::Float(_), ParamValue::Int(v)) => Ok(ParamValue::Float(clamp(*v as f64))),
            (ParamValue::Int(_), ParamValue::Int(v)) => Ok(ParamValue::Int(clamp(*v as f64) as i64)),
            // JSON and UI number fields often carry whole numbers as floats.
            (ParamValue::Int(_), ParamValue::Float(v)) if v.fract() == 0.0 => Ok(ParamValue::Int(clamp(*v) as i64)),
            (ParamValue::Bool(_), ParamValue::Bool(v)) => Ok(ParamValue::Bool(*v)),
            (ParamValue::Curve(_), ParamValue::Curve(pts)) => {
                if pts.len() < 2 {
                    return Err("curve needs at least 2 points".into());
                }
                if pts.windows(2).any(|w| w[1][0] < w[0][0]) || pts.iter().any(|p| !(0.0..=1.0).contains(&p[0])) {
                    return Err("curve x values must be ascending and within 0..1".into());
                }
                Ok(value.clone())
            }
            (ParamValue::Str(_), ParamValue::Str(s)) => {
                if self.hint == Hint::Color && parse_color(s).is_none() {
                    return Err(format!("'{s}' is not a #RRGGBB or #RRGGBBAA color"));
                }
                Ok(value.clone())
            }
            (expected, got) => Err(format!("expected {}, got {}", expected.kind_name(), got.kind_name())),
        }
    }
}

/// Typed access to validated params. Validation guarantees every schema key is present with
/// the right kind, so fallbacks only matter for stages called with unvalidated params.
pub trait ParamsExt {
    fn float(&self, key: &str) -> f32;
    fn int(&self, key: &str) -> i64;
    fn flag(&self, key: &str) -> bool;
    fn curve(&self, key: &str) -> Curve<'_>;
    fn color(&self, key: &str) -> [f32; 4];
}

impl ParamsExt for Params {
    fn float(&self, key: &str) -> f32 {
        match self.get(key) {
            Some(ParamValue::Float(v)) => *v as f32,
            Some(ParamValue::Int(v)) => *v as f32,
            _ => 0.0,
        }
    }
    fn int(&self, key: &str) -> i64 {
        match self.get(key) {
            Some(ParamValue::Int(v)) => *v,
            Some(ParamValue::Float(v)) => *v as i64,
            _ => 0,
        }
    }
    fn flag(&self, key: &str) -> bool {
        matches!(self.get(key), Some(ParamValue::Bool(true)))
    }
    fn curve(&self, key: &str) -> Curve<'_> {
        match self.get(key) {
            Some(ParamValue::Curve(pts)) => Curve(pts),
            _ => Curve(&[[0.0, 0.0], [1.0, 1.0]]),
        }
    }
    fn color(&self, key: &str) -> [f32; 4] {
        match self.get(key) {
            Some(ParamValue::Str(s)) => parse_color(s).unwrap_or([1.0; 4]),
            _ => [1.0; 4],
        }
    }
}

/// Piecewise-linear curve over 0..1.
#[derive(Clone, Copy, Debug)]
pub struct Curve<'a>(pub &'a [[f32; 2]]);

impl Curve<'_> {
    pub fn eval(&self, x: f32) -> f32 {
        let pts = self.0;
        let Some(first) = pts.first() else { return x };
        if x <= first[0] {
            return first[1];
        }
        for w in pts.windows(2) {
            let ([x0, y0], [x1, y1]) = (w[0], w[1]);
            if x <= x1 {
                let t = if x1 > x0 { (x - x0) / (x1 - x0) } else { 1.0 };
                return y0 + (y1 - y0) * t;
            }
        }
        pts[pts.len() - 1][1]
    }
}

/// `#RRGGBB` or `#RRGGBBAA` → linear-ish 0..1 RGBA (sRGB values, as Godot colors expect).
pub fn parse_color(s: &str) -> Option<[f32; 4]> {
    let hex = s.strip_prefix('#')?;
    if !matches!(hex.len(), 6 | 8) || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok().map(|b| b as f32 / 255.0);
    Some([byte(0)?, byte(2)?, byte(4)?, if hex.len() == 8 { byte(6)? } else { 1.0 }])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_interpolates_and_clamps() {
        let c = Curve(&[[0.0, 0.2], [0.5, 0.6], [1.0, 1.0]]);
        assert_eq!(c.eval(-1.0), 0.2);
        assert!((c.eval(0.25) - 0.4).abs() < 1e-6);
        assert_eq!(c.eval(2.0), 1.0);
    }

    #[test]
    fn colors_parse() {
        assert_eq!(parse_color("#FF0000"), Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(parse_color("#00000080").map(|c| (c[3] * 255.0).round()), Some(128.0));
        assert_eq!(parse_color("red"), None);
    }

    #[test]
    fn validation_clamps_coerces_and_rejects() {
        let spec = ParamSpec::float("w", 0.5, 0.0, 1.0, "");
        assert_eq!(spec.validate(&ParamValue::Float(3.0)), Ok(ParamValue::Float(1.0)));
        assert_eq!(spec.validate(&ParamValue::Int(0)), Ok(ParamValue::Float(0.0)));
        assert!(spec.validate(&ParamValue::Bool(true)).is_err());
        let int = ParamSpec::int("n", 2, 0, 4, "");
        assert_eq!(int.validate(&ParamValue::Float(3.0)), Ok(ParamValue::Int(3)));
        assert!(int.validate(&ParamValue::Float(2.5)).is_err());
        let curve = ParamSpec::curve("c", &[[0.0, 0.0], [1.0, 1.0]], "");
        assert!(curve.validate(&ParamValue::Curve(vec![[0.5, 0.0], [0.2, 1.0]])).is_err());
        let color = ParamSpec::color("c", "#000000", "");
        assert!(color.validate(&ParamValue::Str("blue".into())).is_err());
    }
}
