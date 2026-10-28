//! SVG: an orthographic 2D view of the stroke centerlines, in millimeters (plotters, laser
//! cutters, illustration tools). Strokes are filled outlines so pressure-varying width
//! survives; far strokes are drawn first.

use std::fmt::Write as _;

use airdraw_brush::{ParamSpec, ParamValue, Params, params::ParamsExt};
use glam::{Vec2, Vec3};

use crate::exporter::{ExportError, ExportReport, ExportSink, Exporter, ExporterCaps, ExporterInfo};
use crate::scene::{ExportScene, UpAxis};

pub struct Svg;

const VIEWS: [&str; 3] = ["front", "top", "side"];

impl Exporter for Svg {
    fn info(&self) -> ExporterInfo {
        ExporterInfo {
            id: "com.airdraw.svg".into(),
            name: "SVG (2D view)".into(),
            extension: "svg".into(),
            caps: ExporterCaps { curves: true, materials: true, requires_curves: true, ..Default::default() },
        }
    }

    fn options(&self) -> Vec<ParamSpec> {
        vec![
            ParamSpec {
                name: "view",
                default: ParamValue::Str("front".into()),
                min: None,
                max: None,
                hint: airdraw_brush::params::Hint::None,
                doc: "front, top or side",
            },
            ParamSpec::bool("variable_width", true, "Keep pressure-varying width (filled outlines)"),
            ParamSpec::float("width_scale", 1.0, 0.1, 10.0, "Multiply stroke widths"),
        ]
    }

    fn export(&self, scene: &ExportScene, opts: &Params, out: &mut dyn ExportSink) -> Result<ExportReport, ExportError> {
        let view = match opts.get("view") {
            Some(ParamValue::Str(v)) => v.to_lowercase(),
            _ => "front".into(),
        };
        if !VIEWS.contains(&view.as_str()) {
            return Err(ExportError::BadOption { name: "view".into(), reason: format!("'{view}' is not one of {VIEWS:?}") });
        }
        let variable = opts.flag("variable_width");
        let width_scale = opts.float("width_scale");
        // Back to Y-up so view names mean the same whatever the scene's up axis.
        let y_up = |v: Vec3| if scene.meta.up == UpAxis::Z { Vec3::new(v.x, v.z, -v.y) } else { v };
        // (2D in millimeters with y up, depth toward the viewer).
        let project = |v: Vec3| -> (Vec2, f32) {
            let v = y_up(v) * 1000.0;
            match view.as_str() {
                "top" => (Vec2::new(v.x, -v.z), v.y),
                "side" => (Vec2::new(-v.z, v.y), v.x),
                _ => (Vec2::new(v.x, v.y), v.z),
            }
        };

        struct Line {
            pts: Vec<Vec2>,
            widths: Vec<f32>,
            depth: f32,
            material: usize,
        }
        let mut lines: Vec<Line> = scene
            .curves
            .iter()
            .filter(|c| c.points.len() >= 2)
            .map(|c| {
                let projected: Vec<(Vec2, f32)> = c.points.iter().map(|&p| project(p)).collect();
                Line {
                    pts: projected.iter().map(|p| p.0).collect(),
                    widths: c.widths.iter().map(|w| (w * 1000.0 * width_scale).max(0.05)).collect(),
                    depth: projected.iter().map(|p| p.1).sum::<f32>() / projected.len() as f32,
                    material: c.material,
                }
            })
            .collect();
        lines.sort_by(|a, b| a.depth.total_cmp(&b.depth));

        let (mut min, mut max) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for l in &lines {
            for (p, w) in l.pts.iter().zip(&l.widths) {
                min = min.min(*p - *w);
                max = max.max(*p + *w);
            }
        }
        if lines.is_empty() {
            (min, max) = (Vec2::ZERO, Vec2::splat(100.0));
        }
        let margin = 5.0;
        let (min, size) = (min - margin, max - min + 2.0 * margin);
        // SVG's y axis points down.
        let svg = |p: Vec2| Vec2::new(p.x, -p.y);

        let mut s = String::new();
        let _ = writeln!(s, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
        let _ = writeln!(s, "<!-- {} -->", scene.meta.generator);
        let _ = writeln!(
            s,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{:.3}mm" height="{:.3}mm" viewBox="{:.3} {:.3} {:.3} {:.3}">"#,
            size.x,
            size.y,
            min.x,
            -(min.y + size.y),
            size.x,
            size.y
        );
        for l in &lines {
            let m = &scene.materials[l.material];
            let [r, g, b, a] = m.base_color.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
            let color = format!("#{r:02x}{g:02x}{b:02x}");
            let opacity = if a < 255 { format!(r#" opacity="{:.3}""#, a as f32 / 255.0) } else { String::new() };
            if variable {
                let (left, right) = outline(&l.pts, &l.widths);
                let mut d = String::new();
                for (i, p) in left.iter().chain(right.iter().rev()).enumerate() {
                    let p = svg(*p);
                    let _ = write!(d, "{}{:.3},{:.3} ", if i == 0 { "M" } else { "L" }, p.x, p.y);
                }
                let _ = writeln!(s, r#"  <path d="{d}Z" fill="{color}"{opacity}/>"#);
                // Round caps.
                for (p, w) in [(l.pts[0], l.widths[0]), (*l.pts.last().unwrap(), *l.widths.last().unwrap())] {
                    let p = svg(p);
                    let _ = writeln!(s, r#"  <circle cx="{:.3}" cy="{:.3}" r="{:.3}" fill="{color}"{opacity}/>"#, p.x, p.y, w / 2.0);
                }
            } else {
                let mut d = String::new();
                for (i, p) in l.pts.iter().enumerate() {
                    let p = svg(*p);
                    let _ = write!(d, "{}{:.3},{:.3} ", if i == 0 { "M" } else { "L" }, p.x, p.y);
                }
                let avg = l.widths.iter().sum::<f32>() / l.widths.len() as f32;
                let _ = writeln!(
                    s,
                    r#"  <path d="{}" fill="none" stroke="{color}" stroke-width="{avg:.3}" stroke-linecap="round" stroke-linejoin="round"{opacity}/>"#,
                    d.trim_end()
                );
            }
        }
        s += "</svg>\n";
        let name = format!("{}.svg", out.stem());
        out.write(&name, s.as_bytes())?;
        let mut warnings = Vec::new();
        if !scene.meshes.is_empty() && lines.is_empty() {
            warnings.push("No stroke centerlines to draw".into());
        }
        Ok(ExportReport { files: vec![name], warnings })
    }
}

/// Left and right edges of a variable-width polyline.
fn outline(pts: &[Vec2], widths: &[f32]) -> (Vec<Vec2>, Vec<Vec2>) {
    let mut normal = Vec2::Y;
    let mut left = Vec::with_capacity(pts.len());
    let mut right = Vec::with_capacity(pts.len());
    for i in 0..pts.len() {
        let t = pts[(i + 1).min(pts.len() - 1)] - pts[i.saturating_sub(1)];
        if let Some(t) = t.try_normalize() {
            normal = Vec2::new(-t.y, t.x);
        }
        let half = normal * widths[i] / 2.0;
        left.push(pts[i] + half);
        right.push(pts[i] - half);
    }
    (left, right)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exporter::{ExporterRegistry, MemorySink};
    use crate::scene::SceneOptions;
    use crate::scene::tests::{build, sample_doc};

    fn export(opts: Params, scene_opts: SceneOptions) -> Result<String, ExportError> {
        let (doc, lib) = sample_doc();
        let scene = build(&doc, &lib, &SceneOptions { include_curves: true, ..scene_opts });
        let mut sink = MemorySink::new("s");
        ExporterRegistry::with_builtins().export("com.airdraw.svg", &scene, &opts, &mut sink)?;
        Ok(String::from_utf8(sink.files.remove("s.svg").unwrap()).unwrap())
    }

    #[test]
    fn one_outline_per_stroke_with_round_caps() {
        let svg = export(Params::new(), SceneOptions::default()).unwrap();
        assert_eq!(svg.matches("<path ").count(), 3);
        assert_eq!(svg.matches("<circle ").count(), 6);
        assert!(svg.contains(r##"fill="#1e1e24""##) && svg.contains(r##"fill="#2f6fde""##));
        assert!(svg.trim_end().ends_with("</svg>"));
    }

    #[test]
    fn plain_strokes_and_views() {
        let opts: Params = [("variable_width".to_string(), ParamValue::Bool(false))].into();
        let svg = export(opts, SceneOptions::default()).unwrap();
        assert_eq!(svg.matches("stroke-width=").count(), 3);
        assert!(!svg.contains("<circle"));
        // Front and top views of the same strokes differ; Z-up scenes give the same front view.
        let top = export([("view".to_string(), ParamValue::Str("top".into()))].into(), SceneOptions::default()).unwrap();
        let front = export(Params::new(), SceneOptions::default()).unwrap();
        let front_z = export(Params::new(), SceneOptions { up: UpAxis::Z, ..Default::default() }).unwrap();
        assert_ne!(top, front);
        assert_eq!(front, front_z);
    }

    #[test]
    fn bad_view_is_rejected() {
        let e = export([("view".to_string(), ParamValue::Str("diagonal".into()))].into(), SceneOptions::default()).unwrap_err();
        assert!(e.to_string().contains("diagonal"));
    }
}
