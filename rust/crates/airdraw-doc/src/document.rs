//! The document: layers, strokes and the brush snapshots they reference, with undo/redo.
//!
//! Every edit is a reversible [`Op`] and reports [`Change`]s, so views (the Godot canvas,
//! exporters) update only what changed.

use std::sync::Arc;

use airdraw_brush::{BrushDef, InputPoint};
use airdraw_guide::Guide;
use airdraw_look::LookDef;
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
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Op {
    Add {
        index: usize,
        stroke: Stroke,
    },
    /// Strokes with their indices, ascending.
    Remove {
        items: Vec<(usize, Stroke)>,
    },
    SetBrush {
        changes: Vec<(StrokeId, BrushRef, BrushRef)>,
    },
    SetGuide {
        before: Option<Arc<Guide>>,
        after: Option<Arc<Guide>>,
    },
    /// Removes some strokes and adds others in one step: partial erase (strokes → pieces),
    /// transforms (strokes → moved copies) and duplicates (nothing → copies). Changed strokes
    /// get new ids, so a stroke id always means the same points (§3i). `removed` holds indices
    /// before the op and `added` indices after it, both ascending.
    Replace {
        removed: Vec<(usize, Stroke)>,
        added: Vec<(usize, Stroke)>,
    },
}

impl Op {
    /// Strokes this op holds (added or removed ones).
    pub(crate) fn strokes(&self) -> Box<dyn Iterator<Item = &Stroke> + '_> {
        match self {
            Op::Add { stroke, .. } => Box::new(std::iter::once(stroke)),
            Op::Remove { items } => Box::new(items.iter().map(|(_, s)| s)),
            Op::Replace { removed, added } => Box::new(removed.iter().chain(added).map(|(_, s)| s)),
            Op::SetBrush { .. } | Op::SetGuide { .. } => Box::new(std::iter::empty()),
        }
    }

    /// Brush snapshots this op refers to.
    pub(crate) fn brushes(&self) -> Vec<BrushRef> {
        match self {
            Op::SetBrush { changes } => changes.iter().flat_map(|&(_, old, new)| [old, new]).collect(),
            _ => self.strokes().map(|s| s.brush).collect(),
        }
    }
}

pub(crate) const MAX_UNDO: usize = 1000;

#[derive(Debug, Clone)]
pub struct Document {
    pub(crate) layers: Vec<Layer>,
    pub(crate) strokes: Vec<Stroke>,
    /// Brush snapshots referenced by strokes; deduplicated by content.
    pub(crate) brushes: Vec<BrushDef>,
    pub(crate) next_stroke: u64,
    active_layer: LayerId,
    /// Oldest first; saved with the file (format 4, `HIST`).
    pub(crate) undo: Vec<Op>,
    /// The next redo is last.
    pub(crate) redo: Vec<Op>,
    dirty: bool,
    /// PNG preview stored in the file for OS thumbnailers and the app's Recents.
    pub(crate) thumbnail: Option<Arc<[u8]>>,
    /// The drawn 3D guide strokes land on; `None` means the app's plane presets.
    guide: Option<Arc<Guide>>,
    /// The Look strokes are shaded with, as a full copy (like brush snapshots), so a device
    /// without that Look renders it the same; `None` means the app's default.
    pub(crate) look: Option<LookDef>,
    /// Random id given to the sketch when it's created; survives saves and edits. Lets
    /// per-device state (camera, tool) be remembered outside the file (§3h).
    pub(crate) content_id: String,
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
            content_id: new_content_id(),
        }
    }

    /// Builds a loaded document. Undo history starts empty.
    pub(crate) fn from_parts(
        layers: Vec<Layer>,
        strokes: Vec<Stroke>,
        brushes: Vec<BrushDef>,
        thumbnail: Option<Arc<[u8]>>,
        look: Option<LookDef>,
        guide: Option<Guide>,
        content_id: String,
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
            guide: guide.map(Arc::new),
            look,
            content_id,
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

    /// The Look saved with the sketch, if any.
    pub fn look(&self) -> Option<&LookDef> {
        self.look.as_ref()
    }

    /// The saved Look's id, if any.
    pub fn look_id(&self) -> Option<&str> {
        self.look.as_ref().map(|l| l.id.as_str())
    }

    /// Sets the Look. A choice about the artwork's appearance: saved, but not an undo step.
    pub fn set_look(&mut self, look: Option<LookDef>) {
        if self.look != look {
            self.look = look;
            self.dirty = true;
        }
    }

    /// Installs undo/redo history read from a file (already checked against the strokes).
    /// New stroke ids continue after every id the history mentions, so a redo can't collide.
    pub(crate) fn set_history(&mut self, undo: Vec<Op>, redo: Vec<Op>) {
        let max_id = undo.iter().chain(&redo).flat_map(Op::strokes).map(|s| s.id.0).max().unwrap_or(0);
        self.next_stroke = self.next_stroke.max(max_id + 1);
        self.undo = undo;
        self.redo = redo;
    }

    /// The sketch's permanent random id (32 hex digits).
    pub fn content_id(&self) -> &str {
        &self.content_id
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
        let id = self.new_id();
        let stroke = Stroke { id, layer: self.active_layer, brush, points: points.into() };
        let changes = self.commit(Op::Add { index: self.strokes.len(), stroke });
        (id, changes)
    }

    /// Appends several strokes on the active layer as one undo step: one pen stroke that was
    /// drawn in pieces (it left the guide and came back). Returns the changes and the new ids.
    pub fn add_strokes(&mut self, pieces: Vec<(BrushRef, Vec<InputPoint>)>) -> (Vec<Change>, Vec<StrokeId>) {
        let base = self.strokes.len();
        let added: Vec<(usize, Stroke)> = pieces
            .into_iter()
            .enumerate()
            .map(|(n, (brush, points))| (base + n, Stroke { id: self.new_id(), layer: self.active_layer, brush, points: points.into() }))
            .collect();
        if added.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let ids = added.iter().map(|(_, s)| s.id).collect();
        (self.commit(Op::Replace { removed: Vec::new(), added }), ids)
    }

    pub fn remove_strokes(&mut self, ids: &[StrokeId]) -> Vec<Change> {
        let items: Vec<(usize, Stroke)> =
            self.strokes.iter().enumerate().filter(|(_, s)| ids.contains(&s.id)).map(|(i, s)| (i, s.clone())).collect();
        if items.is_empty() {
            return Vec::new();
        }
        self.commit(Op::Remove { items })
    }

    /// Replaces each stroke with new ones made of the given points, in its place and with its
    /// layer and brush: no pieces deletes it, one moves it, several split it. One undo step.
    /// Returns the changes and each new stroke's id, in the order given.
    pub fn replace_strokes(&mut self, replacements: Vec<(StrokeId, Vec<Vec<InputPoint>>)>) -> (Vec<Change>, Vec<StrokeId>) {
        let mut seen = std::collections::HashSet::new();
        let mut targets: Vec<(usize, Stroke, Vec<Vec<InputPoint>>)> = replacements
            .into_iter()
            .filter_map(|(id, pieces)| {
                let i = self.strokes.iter().position(|s| s.id == id && seen.insert(id))?;
                Some((i, self.strokes[i].clone(), pieces))
            })
            .collect();
        if targets.is_empty() {
            return (Vec::new(), Vec::new());
        }
        // New ids in the caller's order; positions in document order.
        let mut ids = Vec::new();
        for (_, _, pieces) in &targets {
            ids.push((0..pieces.len()).map(|_| self.new_id()).collect::<Vec<_>>());
        }
        let mut order: Vec<usize> = (0..targets.len()).collect();
        order.sort_by_key(|&k| targets[k].0);
        let (mut removed, mut added) = (Vec::new(), Vec::new());
        let mut shift = 0isize;
        for k in order {
            let (index, stroke, pieces) = (targets[k].0, targets[k].1.clone(), std::mem::take(&mut targets[k].2));
            let at = (index as isize + shift) as usize;
            for (n, (points, &id)) in pieces.into_iter().zip(&ids[k]).enumerate() {
                added.push((at + n, Stroke { id, points: points.into(), ..stroke.clone() }));
            }
            shift += ids[k].len() as isize - 1;
            removed.push((index, stroke));
        }
        let changes = self.commit(Op::Replace { removed, added });
        (changes, ids.into_iter().flatten().collect())
    }

    /// Copies strokes (same layer and brush, points moved by `offset`) on top of the sketch, in
    /// document order. One undo step. Returns the changes and the copies' ids.
    pub fn duplicate_strokes(&mut self, ids: &[StrokeId], offset: glam::Vec3) -> (Vec<Change>, Vec<StrokeId>) {
        let originals: Vec<Stroke> = self.strokes.iter().filter(|s| ids.contains(&s.id)).cloned().collect();
        if originals.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let base = self.strokes.len();
        let added: Vec<(usize, Stroke)> = originals
            .into_iter()
            .enumerate()
            .map(|(n, s)| {
                let points: Arc<[InputPoint]> = if offset == glam::Vec3::ZERO {
                    s.points.clone()
                } else {
                    s.points.iter().map(|p| InputPoint { pos: p.pos + offset, ..*p }).collect()
                };
                (base + n, Stroke { id: self.new_id(), points, ..s })
            })
            .collect();
        let new_ids = added.iter().map(|(_, s)| s.id).collect();
        (self.commit(Op::Replace { removed: Vec::new(), added }), new_ids)
    }

    /// Moves strokes by `xf` (positions transformed, normals rotated with it), each keeping its
    /// place, layer and brush but getting a new id. One undo step. Returns the changes and the
    /// moved strokes' new ids, in the order given. `xf` should be rigid (move and rotate):
    /// strokes have no size of their own yet (§3i).
    pub fn transform_strokes(&mut self, ids: &[StrokeId], xf: glam::Affine3A) -> (Vec<Change>, Vec<StrokeId>) {
        let rot = glam::Mat3::from(xf.matrix3);
        let replacements = ids
            .iter()
            .filter_map(|id| {
                let s = self.stroke(*id)?;
                let moved = s
                    .points
                    .iter()
                    .map(|p| InputPoint {
                        pos: xf.transform_point3(p.pos),
                        normal: (rot * p.normal).try_normalize().unwrap_or(p.normal),
                        ..*p
                    })
                    .collect();
                Some((*id, vec![moved]))
            })
            .collect();
        self.replace_strokes(replacements)
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

    fn new_id(&mut self) -> StrokeId {
        self.next_stroke += 1;
        StrokeId(self.next_stroke - 1)
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
            Op::Remove { items } => self.take(items),
            Op::Replace { removed, added } => {
                let mut changes = self.take(removed);
                changes.extend(self.put(added));
                changes
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
            Op::Remove { items } => self.put(items),
            Op::Replace { removed, added } => {
                let mut changes = self.take(added);
                changes.extend(self.put(removed));
                changes
            }
            Op::SetBrush { changes } => self.set_refs(changes.iter().map(|&(id, old, _)| (id, old))),
            Op::SetGuide { before, .. } => {
                self.guide = before.clone();
                vec![Change::Guide]
            }
        }
    }

    /// Removes strokes at ascending indices, from the back so earlier indices stay valid.
    fn take(&mut self, items: &[(usize, Stroke)]) -> Vec<Change> {
        for (i, _) in items.iter().rev() {
            self.strokes.remove(*i);
        }
        items.iter().map(|(_, s)| Change::Removed(s.id)).collect()
    }

    /// Inserts strokes at ascending indices (each index is where it ends up).
    fn put(&mut self, items: &[(usize, Stroke)]) -> Vec<Change> {
        for (i, s) in items {
            self.strokes.insert((*i).min(self.strokes.len()), s.clone());
        }
        items.iter().map(|(_, s)| Change::Added(s.id)).collect()
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

/// 128 random bits as hex. `RandomState` is seeded from the OS's randomness per instance, so
/// this needs no extra dependency; the time and a counter only make repeats even less likely.
pub(crate) fn new_content_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let half = || {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(nanos);
        h.write_u64(n);
        h.finish()
    };
    format!("{:016x}{:016x}", half(), half())
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
    fn add_strokes_is_one_undo_step() {
        let mut d = Document::new();
        let ink = d.intern_brush(&brush("com.airdraw.ink"));
        d.add_stroke(ink, points(5, 0.0));
        let (_, ids) = d.add_strokes(vec![(ink, points(4, 1.0)), (ink, points(6, 2.0))]);
        assert_eq!(ids.len(), 2);
        assert_eq!(d.strokes().iter().map(|s| s.id).skip(1).collect::<Vec<_>>(), ids);
        d.undo();
        assert_eq!(d.strokes().len(), 1);
        d.redo();
        assert_eq!(d.strokes().len(), 3);
        assert_eq!(d.add_strokes(Vec::new()).1, Vec::new());
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

    fn ids(d: &Document) -> Vec<StrokeId> {
        d.strokes().iter().map(|s| s.id).collect()
    }

    #[test]
    fn replace_splits_moves_and_deletes_in_place() {
        let mut d = Document::new();
        let ink = d.intern_brush(&brush("com.airdraw.ink"));
        let ribbon = d.intern_brush(&brush("com.airdraw.ribbon"));
        let s: Vec<_> = (0..4).map(|i| d.add_stroke(if i == 2 { ribbon } else { ink }, points(6, i as f32)).0).collect();
        let before = d.strokes().to_vec();
        let p = points(6, 9.0);
        // Caller's order isn't document order: stroke 2 splits in two, 0 moves, 3 goes.
        let (changes, new) =
            d.replace_strokes(vec![(s[2], vec![p[..2].to_vec(), p[3..].to_vec()]), (s[0], vec![p.clone()]), (s[3], vec![])]);
        assert_eq!(new.len(), 3);
        assert_eq!(ids(&d), vec![new[2], s[1], new[0], new[1]], "pieces take their stroke's place");
        assert_eq!(d.stroke(new[0]).unwrap().brush, ribbon, "pieces keep the brush");
        assert_eq!(&*d.stroke(new[1]).unwrap().points, &p[3..]);
        assert_eq!(&changes[..3], &[Change::Removed(s[0]), Change::Removed(s[2]), Change::Removed(s[3])]);
        assert_eq!(changes.len(), 6);
        // One undo step, exact both ways.
        let after = d.strokes().to_vec();
        d.undo();
        assert_eq!(d.strokes(), &before[..]);
        d.redo();
        assert_eq!(d.strokes(), &after[..]);
        // Ids are never reused.
        let (fresh, _) = d.add_stroke(ink, points(2, 0.0));
        assert!(fresh.0 > new.iter().map(|i| i.0).max().unwrap());
        assert_eq!(d.replace_strokes(vec![(StrokeId(999), vec![])]), (vec![], vec![]), "unknown strokes: no edit");
    }

    #[test]
    fn pieces_after_a_split_land_after_its_pieces() {
        let mut d = Document::new();
        let ink = d.intern_brush(&brush("com.airdraw.ink"));
        let s: Vec<_> = (0..3).map(|i| d.add_stroke(ink, points(4, i as f32)).0).collect();
        let (_, new) = d.replace_strokes(vec![(s[0], vec![points(2, 5.0), points(2, 6.0), points(2, 7.0)]), (s[2], vec![points(2, 8.0)])]);
        assert_eq!(ids(&d), vec![new[0], new[1], new[2], s[1], new[3]]);
        d.undo();
        assert_eq!(ids(&d), s);
    }

    #[test]
    fn replace_ignores_a_stroke_named_twice() {
        let mut d = Document::new();
        let ink = d.intern_brush(&brush("com.airdraw.ink"));
        let (a, _) = d.add_stroke(ink, points(4, 0.0));
        let (_, new) = d.replace_strokes(vec![(a, vec![points(3, 1.0)]), (a, vec![points(3, 2.0)])]);
        assert_eq!(ids(&d), new);
        assert_eq!(new.len(), 1);
    }

    #[test]
    fn duplicates_go_on_top_and_undo_in_one_step() {
        let mut d = Document::new();
        let ink = d.intern_brush(&brush("com.airdraw.ink"));
        let s: Vec<_> = (0..3).map(|i| d.add_stroke(ink, points(4, i as f32)).0).collect();
        let (changes, copies) = d.duplicate_strokes(&[s[2], s[0]], Vec3::ZERO);
        assert_eq!(changes, copies.iter().map(|&c| Change::Added(c)).collect::<Vec<_>>());
        assert_eq!(ids(&d), [s.clone(), copies.clone()].concat());
        assert_eq!(d.stroke(copies[0]).unwrap().points, d.stroke(s[0]).unwrap().points, "document order");
        d.undo();
        assert_eq!(ids(&d), s);
        assert!(d.duplicate_strokes(&[], Vec3::ZERO).0.is_empty());
        let (_, moved) = d.duplicate_strokes(&[s[1]], Vec3::X);
        let (a, b) = (&d.stroke(s[1]).unwrap().points, &d.stroke(moved[0]).unwrap().points);
        assert!(a.iter().zip(b.iter()).all(|(p, q)| q.pos == p.pos + Vec3::X && q.pressure == p.pressure));
    }

    #[test]
    fn transforms_bake_into_points_as_one_step() {
        let mut d = Document::new();
        let ink = d.intern_brush(&brush("com.airdraw.ink"));
        let s: Vec<_> = (0..3).map(|i| d.add_stroke(ink, points(5, i as f32)).0).collect();
        let before = d.strokes().to_vec();
        let xf =
            glam::Affine3A::from_rotation_translation(glam::Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), Vec3::new(1.0, 2.0, 3.0));
        let (changes, moved) = d.transform_strokes(&[s[2], s[0]], xf);
        assert_eq!(changes.len(), 4);
        assert_eq!(ids(&d), vec![moved[1], s[1], moved[0]], "each keeps its place");
        let (a, b) = (&before[0].points, &d.stroke(moved[1]).unwrap().points);
        for (p, q) in a.iter().zip(b.iter()) {
            assert!(q.pos.distance(xf.transform_point3(p.pos)) < 1e-6);
            assert!(q.normal.distance(Vec3::new(1.0, 0.0, 0.0)) < 1e-6, "Z turned a quarter about Y is X");
            assert_eq!((q.pressure, q.tilt, q.t), (p.pressure, p.tilt, p.t));
        }
        d.undo();
        assert_eq!(d.strokes(), &before[..], "undo is exact");
        assert!(d.transform_strokes(&[StrokeId(77)], xf).0.is_empty());
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
