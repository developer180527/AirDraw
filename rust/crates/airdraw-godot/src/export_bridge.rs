//! Converts between Godot values and parameter schemas / brush definitions, so GDScript can
//! generate editors (export dialog, brush editor) from schemas and hand values back.

use airdraw_brush::params::Hint;
use airdraw_brush::{BrushDef, ParamSpec, ParamValue, Params, Slot, StageRef, StageRegistry, brush::resolve};
use godot::prelude::*;

/// Schema entry → `{name, kind, default, min, max, doc, group}`.
/// `kind` is one of bool, int, float, color, string, curve (default: PackedVector2Array).
pub fn spec_to_dict(spec: &ParamSpec, group: &str) -> VarDictionary {
    let mut d = VarDictionary::new();
    let kind = match (&spec.default, spec.hint) {
        (ParamValue::Bool(_), _) => "bool",
        (ParamValue::Int(_), _) => "int",
        (ParamValue::Float(_), _) => "float",
        (ParamValue::Str(_), Hint::Color) => "color",
        (ParamValue::Str(_), _) => "string",
        (ParamValue::Curve(_), _) => "curve",
    };
    d.set("name", spec.name);
    d.set("kind", kind);
    d.set("default", &param_to_variant(&spec.default, spec.hint));
    d.set("min", &spec.min.map_or(Variant::nil(), |v| v.to_variant()));
    d.set("max", &spec.max.map_or(Variant::nil(), |v| v.to_variant()));
    d.set("doc", spec.doc);
    d.set("group", group);
    d
}

pub fn param_to_variant(v: &ParamValue, hint: Hint) -> Variant {
    match v {
        ParamValue::Bool(b) => b.to_variant(),
        ParamValue::Int(i) => i.to_variant(),
        ParamValue::Float(f) => f.to_variant(),
        ParamValue::Str(s) if hint == Hint::Color => Color::from_html(s).unwrap_or(Color::WHITE).to_variant(),
        ParamValue::Str(s) => s.to_variant(),
        ParamValue::Curve(pts) => pts.iter().map(|p| Vector2::new(p[0], p[1])).collect::<PackedVector2Array>().to_variant(),
    }
}

/// Godot value → parameter. With a spec, numbers are coerced to the declared kind (GDScript
/// range controls always produce floats).
pub fn variant_to_param(v: &Variant, spec: Option<&ParamSpec>) -> Option<ParamValue> {
    let ty = v.get_type();
    let number = || match ty {
        VariantType::INT => Some(v.to::<i64>() as f64),
        VariantType::FLOAT => Some(v.to::<f64>()),
        _ => None,
    };
    match spec.map(|s| &s.default) {
        Some(ParamValue::Int(_)) => return number().map(|n| ParamValue::Int(n.round() as i64)),
        Some(ParamValue::Float(_)) => return number().map(ParamValue::Float),
        _ => {}
    }
    Some(match ty {
        VariantType::BOOL => ParamValue::Bool(v.to::<bool>()),
        VariantType::INT => ParamValue::Int(v.to::<i64>()),
        VariantType::FLOAT => ParamValue::Float(v.to::<f64>()),
        VariantType::STRING => ParamValue::Str(v.to::<GString>().to_string()),
        VariantType::COLOR => ParamValue::Str(format!("#{}", v.to::<Color>().to_html())),
        VariantType::PACKED_VECTOR2_ARRAY => {
            ParamValue::Curve(v.to::<PackedVector2Array>().as_slice().iter().map(|p| [p.x, p.y]).collect())
        }
        _ => return None,
    })
}

/// Dialog values → params. Unsupported types are skipped; callers validate against schemas.
pub fn dict_to_params(d: &VarDictionary) -> Params {
    d.iter_shared().filter_map(|(k, v)| Some((k.to_string(), variant_to_param(&v, None)?))).collect()
}

pub fn slot_from_name(name: &str) -> Option<Slot> {
    Some(match name {
        "filters" => Slot::Filter,
        "dynamics" => Slot::Dynamics,
        "geometry" => Slot::Geometry,
        "material" => Slot::Material,
        _ => return None,
    })
}

/// Brush → `{id, name, filters: [{type, params}], dynamics: [...], geometry: {type, params},
/// material: {type, params}}`, with every parameter filled in (schema defaults included).
pub fn def_to_dict(def: &BrushDef, reg: &StageRegistry) -> VarDictionary {
    let stage = |slot: Slot, r: &StageRef| {
        let schema = reg.schema(slot, &r.kind).unwrap_or_default();
        let params = resolve(r, &schema).unwrap_or_else(|_| r.params.clone());
        let mut p = VarDictionary::new();
        for (name, value) in &params {
            let hint = schema.iter().find(|s| s.name == name).map_or(Hint::None, |s| s.hint);
            p.set(name.as_str(), &param_to_variant(value, hint));
        }
        let mut d = VarDictionary::new();
        d.set("type", r.kind.as_str());
        d.set("params", &p);
        d
    };
    let mut d = VarDictionary::new();
    d.set("id", def.id.as_str());
    d.set("name", def.name.as_str());
    d.set("filters", &def.filters.iter().map(|r| stage(Slot::Filter, r)).collect::<Array<VarDictionary>>());
    d.set("dynamics", &def.dynamics.iter().map(|r| stage(Slot::Dynamics, r)).collect::<Array<VarDictionary>>());
    d.set("geometry", &stage(Slot::Geometry, &def.geometry));
    d.set("material", &stage(Slot::Material, &def.material));
    d
}

/// Inverse of [`def_to_dict`]. Structural problems are errors; parameter problems are left
/// for `Brush::compile` to report with stage and parameter names.
pub fn dict_to_def(d: &VarDictionary, reg: &StageRegistry) -> Result<BrushDef, String> {
    let text = |key: &str| d.get(key).map(|v| v.to::<GString>().to_string()).ok_or_else(|| format!("missing '{key}'"));
    let stage = |slot: Slot, v: Variant| -> Result<StageRef, String> {
        let sd = v.try_to::<VarDictionary>().map_err(|_| "stage must be a dictionary".to_string())?;
        let kind = sd.get("type").map(|t| t.to::<GString>().to_string()).ok_or("stage without 'type'")?;
        let schema = reg.schema(slot, &kind).unwrap_or_default();
        let params = sd.get("params").and_then(|p| p.try_to::<VarDictionary>().ok()).unwrap_or_default();
        let params = params
            .iter_shared()
            .filter_map(|(k, v)| {
                let name = k.to_string();
                let spec = schema.iter().find(|s| s.name == name);
                Some((name, variant_to_param(&v, spec)?))
            })
            .collect();
        Ok(StageRef { kind, params })
    };
    let list = |slot: Slot, key: &str| -> Result<Vec<StageRef>, String> {
        let arr = d.get(key).and_then(|v| v.try_to::<VarArray>().ok()).unwrap_or_default();
        arr.iter_shared().map(|v| stage(slot, v)).collect()
    };
    Ok(BrushDef {
        schema: airdraw_brush::brush::SCHEMA_VERSION,
        id: text("id")?,
        name: text("name")?,
        icon: None,
        filters: list(Slot::Filter, "filters")?,
        dynamics: list(Slot::Dynamics, "dynamics")?,
        geometry: stage(Slot::Geometry, d.get("geometry").ok_or("missing 'geometry'")?)?,
        material: stage(Slot::Material, d.get("material").ok_or("missing 'material'")?)?,
    })
}
