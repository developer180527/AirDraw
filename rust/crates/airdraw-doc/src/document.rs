//! The document: layers, strokes and the brush snapshots they reference, with undo/redo.
//!
//! Every edit is a reversible [`Op`] and reports [`Change`]s, so views (the Godot canvas,
//! exporters) update only what changed.

use std::sync::Arc;

use airdraw_brush::{BrushDef, InputPoint};
use airdraw_guide::Guide;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StrokeId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LayerId(pub u32);

/// Index into the document's brush snapshot table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BrushRef(pub u32);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
}

/// A stroke keeps its raw input and the exact brush settings it was drawn with, so it can be
/// rebuilt identically later, re-brushed, or re-meshed at another quality for export.
#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub id: StrokeId,
    pub layer: LayerId,
    pub brush: BrushRef,
    /// Shared so undo history and the document don't duplicate point data.
    pub points: Arc<[InputPoint]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Added(StrokeId),
    Removed(StrokeId),
    /// Same stroke, different brush or points: rebuild its mesh.
    Updated(StrokeId),
    /// Everything changed (document loaded or replaced): rebuild all.
    Reset,
    /// The active 3D guide was set, replaced or removed.
    Guide,
}

/// A reversible edit. Holds what's needed to apply and revert it.
#[derive(Clone, Debug)]
enum Op {
    Add { index: usize, stroke: Stroke },
    /// Strokes with their indices, ascending.
    Remove { items: Vec<(usize, Stroke)> },
    SetBrush { changes: Vec<(StrokeId, BrushRef, BrushRef)> },
    SetGuide { before: Option<Arc<Guide>>, after: Option<Arc<Guide>> },
}

const MAX_UNDO: usize = 1000;

#[derive(Debug, Clone)]
pub struct Document {
    pub(crate) layers: Vec<Layer>,
    pub(crate) strokes: Vec<Stroke>,
    /// Brush snapshots referenced by strokes; deduplicated by content.
    pub(crate) brushes: Vec<BrushDef>,
    pub(crate) next_stroke: u64,
    active_layer: LayerId,
    undo: Vec<Op>,
    redo: Vec<Op>,
    dirty: bool,
    /// PNG preview stored in the file for OS thumbnailers and the app's Recents.
    pub(crate) thumbnail: Option<Arc<[u8]>>,
    /// The drawn 3D guide strokes land on; `None` means the app's plane presets.
    guide: Option<Arc<Guide>>,
    /// Id of the Look strokes are shaded with; `None` means the app's default.
    pub(crate) look: Option<String>,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    pub fn new() -> Self {
        Self {
            layers: vec![Layer { id: LayerId(1), name: "Layer 1".into(), visible: true, locked: false }],
            strokes: Vec::new(),
            brushes: Vec::new(),
            next_stroke: 1,
            active_layer: LayerId(1),
            undo: Vec::new(),
            redo: Vec::new(),
            dirty: false,
            thumbnail: None,
            guide: None,
            look: None,
        }
    }

    /// Builds a loaded document. Undo history starts empty.
    pub(crate) fn from_parts(
        layers: Vec<Layer>,
        strokes: Vec<Stroke>,
        brushes: Vec<BrushDef>,
        thumbnail: Option<Arc<[u8]>>,
        look: Option<String>,
    ) -> Self {
        let next_stroke = strokes.iter().map(|s| s.id.0 + 1).max().unwrap_or(1);
        let active_layer = layers.first().map_or(LayerId(1), |l| l.id);
        Self {
            layers,
            strokes,
            brushes,
            next_stroke,
            active_layer,
            undo: Vec::new(),
            redo: Vec::new(),
            dirty: false,
            thumbnail,
            guide: None,
            look,
        }
    }

    // ---- Queries -----------------------------------------------------------------------

    pub fn strokes(&self) -> &[Stroke] {
        &self.strokes
    }

    pub fn stroke(&self, id: StrokeId) -> Option<&Stroke> {
        self.strokes.iter().find(|s| s.id == id)
    }

    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    pub fn brush_def(&self, r: BrushRef) -> Option<&BrushDef> {
        self.brushes.get(r.0 as usize)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// True if there are edits since the last load/save.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// The Look id saved with the sketch, if any.
    pub fn look(&self) -> Option<&str> {
        self.look.as_deref()
    }

    /// Sets the Look. A view choice like layer visibility: saved, but not an undo step.
    pub fn set_look(&mut self, look: Option<String>) {
        if self.look != look {
            self.look = look;
            self.dirty = true;
        }
    }

    /// The active drawn guide, if any (otherwise strokes use the app's plane presets).
    pub fn guide(&self) -> Option<&Arc<Guide>> {
        self.guide.as_ref()
    }

    /// The embedded PNG preview, if any.
    pub fn thumbnail(&self) -> Option<&[u8]> {
        self.thumbnail.as_deref()
    }

    /// Sets the PNG preview written with the next save. A preview isn't an edit: it doesn't
    /// mark the document dirty or enter undo history.
    pub fn set_thumbnail(&mut self, png: Option<Vec<u8>>) -> Result<(), crate::DocError> {
        if let Some(p) = &png {
            crate::file::check_thumbnail(p)?;
        }
        self.thumbnail = png.map(Into::into);
        Ok(())
    }

    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    // ---- Edits -------------------------------------------------------------------------

    /// Shows or hides a layer. Visibility is a view setting, so it isn't recorded for undo.
    pub fn set_layer_visible(&mut self, id: LayerId, visible: bool) {
        if let Some(l) = self.layers.iter_mut().find(|l| l.id == id)
            && l.visible != visible
        {
            l.visible = visible;
            self.dirty = true;
        }
    }

    /// Returns the snapshot slot for `def`, adding it if no identical snapshot exists.
    /// Snapshots aren't part of undo history; unused ones are dropped when saving.
    pub fn intern_brush(&mut self, def: &BrushDef) -> BrushRef {
        let idx = match self.brushes.iter().position(|b| b == def) {
            Some(i) => i,
            None => {
                self.brushes.push(def.clone());
                self.brushes.len() - 1
            }
        };
        BrushRef(idx as u32)
    }

    /// Appends a stroke on the active layer.
    pub fn add_stroke(&mut self, brush: BrushRef, points: Vec<InputPoint>) -> (StrokeId, Vec<Change>) {
        let id = StrokeId(self.next_stroke);
        self.next_stroke += 1;
        let stroke = Stroke { id, layer: self.active_layer, brush, points: points.into() };
        let changes = self.commit(Op::Add { index: self.strokes.len(), stroke });
        (id, changes)
    }

    pub fn remove_strokes(&mut self, ids: &[StrokeId]) -> Vec<Change> {
        let items: Vec<(usize, Stroke)> =
            self.strokes.iter().enumerate().filter(|(_, s)| ids.contains(&s.id)).map(|(i, s)| (i, s.clone())).collect();
        if items.is_empty() {
            return Vec::new();
        }
        self.commit(Op::Remove { items })
    }

    pub fn clear(&mut self) -> Vec<Change> {
        let ids: Vec<StrokeId> = self.strokes.iter().map(|s| s.id).collect();
        self.remove_strokes(&ids)
    }

    /// Re-brushes strokes: same raw input, new brush.
    pub fn set_brush(&mut self, ids: &[StrokeId], brush: BrushRef) -> Vec<Change> {
        let changes: Vec<_> =
            self.strokes.iter().filter(|s| ids.contains(&s.id) && s.brush != brush).map(|s| (s.id, s.brush, brush)).collect();
        if changes.is_empty() {
            return Vec::new();
        }
        self.commit(Op::SetBrush { changes })
    }

    /// Replaces the active guide (`None` removes it). Undoable, like strokes.
    pub fn set_guide(&mut self, guide: Option<Guide>) -> Vec<Change> {
        if guide.is_none() && self.guide.is_none() {
            return Vec::new();
        }
        self.commit(Op::SetGuide { before: self.guide.clone(), after: guide.map(Arc::new) })
    }

    pub fn undo(&mut self) -> Vec<Change> {
        let Some(op) = self.undo.pop() else { return Vec::new() };
        let changes = self.revert(&op);
        self.redo.push(op);
        self.dirty = true;
        changes
    }

    pub fn redo(&mut self) -> Vec<Change> {
        let Some(op) = self.redo.pop() else { return Vec::new() };
        let changes = self.apply(&op);
        self.undo.push(op);
        self.dirty = true;
        changes
    }

    fn commit(&mut self, op: Op) -> Vec<Change> {
        let changes = self.apply(&op);
        self.undo.push(op);
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.dirty = true;
        changes
    }

    fn apply(&mut self, op: &Op) -> Vec<Change> {
        match op {
            Op::Add { index, stroke } => {
                self.strokes.insert((*index).min(self.strokes.len()), stroke.clone());
                vec![Change::Added(stroke.id)]
            }
            Op::Remove { items } => {
                // Remove from the back so earlier indices stay valid.
                for (i, _) in items.iter().rev() {
                    self.strokes.remove(*i);
                }
                items.iter().map(|(_, s)| Change::Removed(s.id)).collect()
            }
            Op::SetBrush { changes } => self.set_refs(changes.iter().map(|&(id, _, new)| (id, new))),
            Op::SetGuide { after, .. } => {
                self.guide = after.clone();
                vec![Change::Guide]
            }
        }
    }

    fn revert(&mut self, op: &Op) -> Vec<Change> {
        match op {
            Op::Add { stroke, .. } => {
                self.strokes.retain(|s| s.id != stroke.id);
                vec![Change::Removed(stroke.id)]
            }
            Op::Remove { items } => {
                for (i, s) in items {
                    self.strokes.insert((*i).min(self.strokes.len()), s.clone());
                }
                items.iter().map(|(_, s)| Change::Added(s.id)).collect()
            }
            Op::SetBrush { changes } => self.set_refs(changes.iter().map(|&(id, old, _)| (id, old))),
            Op::SetGuide { before, .. } => {
                self.guide = before.clone();
                vec![Change::Guide]
            }
        }
    }

    fn set_refs(&mut self, refs: impl Iterator<Item = (StrokeId, BrushRef)>) -> Vec<Change> {
        let mut out = Vec::new();
        for (id, r) in refs {
            if let Some(s) = self.strokes.iter_mut().find(|s| s.id == id) {
                s.brush = r;
                out.push(Change::Updated(id));
            }
        }
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use airdraw_brush::{BrushLibrary, StageRegistry};
    use glam::Vec3;

    pub fn points(n: usize, y: f32) -> Vec<InputPoint> {
        (0..n)
            .map(|i| InputPoint {
                pos: Vec3::new(i as f32 * 0.01, y, 0.0),
                normal: Vec3::Z,
                pressure: 0.5 + i as f32 * 0.01,
                tilt: if i % 2 == 0 { Some(0.7) } else { None },
                t: 10.0 + i as f64 / 240.0,
            })
            .collect()
    }

    pub fn brush(id: &str) -> BrushDef {
        BrushLibrary::new(StageRegistry::with_builtins()).get(id).unwrap().def.clone()
    }

    #[test]
    fn add_undo_redo() {
        let mut d = Document::new();
        let b = d.intern_brush(&brush("com.airdraw.ink"));
        let (a, ch) = d.add_stroke(b, points(5, 0.0));
        assert_eq!(ch, vec![Change::Added(a)]);
        let (b2, _) = d.add_stroke(b, points(5, 1.0));
        assert_eq!(d.undo(), vec![Change::Removed(b2)]);
        assert_eq!(d.strokes().len(), 1);
        assert_eq!(d.redo(), vec![Change::Added(b2)]);
        assert_eq!(d.strokes().iter().map(|s| s.id).collect::<Vec<_>>(), vec![a, b2]);
    }

    #[test]
    fn guide_set_remove_undo() {
        let mut d = Document::new();
        assert!(d.set_guide(None).is_empty(), "removing nothing is no edit");
        let g = Guide::Plane { origin: Vec3::ZERO, normal: Vec3::Y };
        assert_eq!(d.set_guide(Some(g.clone())), vec![Change::Guide]);
        assert_eq!(d.guide().map(|g| (**g).clone()), Some(g.clone()));
        assert_eq!(d.set_guide(None), vec![Change::Guide]);
        assert!(d.guide().is_none());
        assert_eq!(d.undo(), vec![Change::Guide]);
        assert_eq!(d.guide().map(|g| (**g).clone()), Some(g));
        d.undo();
        assert!(d.guide().is_none() && !d.can_undo());
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut d = Document::new();
        let b = d.intern_brush(&brush("com.airdraw.ink"));
        d.add_stroke(b, points(3, 0.0));
        d.undo();
        assert!(d.can_redo());
        d.add_stroke(b, points(3, 0.0));
        assert!(!d.can_redo());
    }

    #[test]
    fn clear_undo_restores_order() {
        let mut d = Document::new();
        let b = d.intern_brush(&brush("com.airdraw.ink"));
        let ids: Vec<_> = (0..4).map(|i| d.add_stroke(b, points(3, i as f32)).0).collect();
        d.remove_strokes(&[ids[1]]);
        d.clear();
        assert!(d.strokes().is_empty());
        d.undo();
        assert_eq!(d.strokes().iter().map(|s| s.id).collect::<Vec<_>>(), vec![ids[0], ids[2], ids[3]]);
        d.undo();
        assert_eq!(d.strokes().iter().map(|s| s.id).collect::<Vec<_>>(), ids);
    }

    #[test]
    fn rebrush_is_undoable_and_keeps_points() {
        let mut d = Document::new();
        let ink = d.intern_brush(&brush("com.airdraw.ink"));
        let ribbon = d.intern_brush(&brush("com.airdraw.ribbon"));
        let (id, _) = d.add_stroke(ink, points(5, 0.0));
        let before = d.stroke(id).unwrap().points.clone();
        assert_eq!(d.set_brush(&[id], ribbon), vec![Change::Updated(id)]);
        assert_eq!(d.stroke(id).unwrap().brush, ribbon);
        assert!(Arc::ptr_eq(&d.stroke(id).unwrap().points, &before));
        d.undo();
        assert_eq!(d.stroke(id).unwrap().brush, ink);
        assert!(d.set_brush(&[id], ink).is_empty(), "no-op re-brush records nothing");
    }

    #[test]
    fn identical_brushes_share_a_snapshot() {
        let mut d = Document::new();
        let a = d.intern_brush(&brush("com.airdraw.ink"));
        let b = d.intern_brush(&brush("com.airdraw.ink"));
        let mut tweaked = brush("com.airdraw.ink");
        tweaked.name = "Ink 2".into();
        let c = d.intern_brush(&tweaked);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn dirty_tracking() {
        let mut d = Document::new();
        assert!(!d.is_dirty());
        let b = d.intern_brush(&brush("com.airdraw.ink"));
        assert!(!d.is_dirty(), "interning alone is not an edit");
        d.add_stroke(b, points(3, 0.0));
        assert!(d.is_dirty());
        d.mark_saved();
        d.undo();
        assert!(d.is_dirty());
    }
}
