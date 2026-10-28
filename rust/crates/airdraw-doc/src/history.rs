//! Undo/redo history in format 4 (the optional `HIST` section, docs/ARCHITECTURE.md §3h).
//!
//! ```text
//! u32 TOML length, TOML: the ops (undo oldest first, redo with the next redo last)
//! pool: strokes only history holds (deleted ones, undone adds), encoded like `STRK`
//! ```
//!
//! ```toml
//! [[undo]]
//! op = "add"
//! index = 3
//! stroke = [17, 2]          # stroke id, brush
//! [[undo]]
//! op = "remove"
//! items = [[0, 4, 1], [5, 9, 0]]   # index, stroke id, brush
//! [[undo]]
//! op = "set_brush"
//! changes = [[4, 1, 0]]     # stroke id, old brush, new brush
//! [[undo]]
//! op = "replace"            # partial erase, transform, duplicate (§3i)
//! removed = [[2, 5, 0]]     # index before, stroke id, brush
//! added = [[2, 11, 0], [3, 12, 0]]  # index after, stroke id, brush
//! [[undo]]
//! op = "set_guide"
//! after = { kind = "plane", origin = [0, 0, 0], normal = [0, 0, 1] }
//! ```
//!
//! A stroke's points are stored once: from `STRK` if it's in the sketch, else from the pool.
//! Each reference carries its own brush, since re-brushing changes a stroke's brush over
//! time. Brush numbers index `META`'s snapshot list, where snapshots only history uses
//! follow the live ones.
//!
//! History is optional data: anything wrong with it (bad references, ops that don't fit the
//! strokes) drops the history, never the sketch.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::DocError;
use crate::document::{BrushRef, Op, Stroke, StrokeId};
use crate::file::GuideDef;

/// History keeps at most the sketch's peak size during the history (or this much, if
/// larger) of its own data: pool points before compression.
pub(crate) const MIN_BUDGET: usize = 1 << 20;
/// Rough raw bytes per stored point (position, normal, pressure, tilt, time).
const POINT_BYTES: usize = 40;

#[derive(Serialize, Deserialize)]
struct HistDef {
    #[serde(default)]
    undo: Vec<OpDef>,
    #[serde(default)]
    redo: Vec<OpDef>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum OpDef {
    Add {
        index: u64,
        stroke: [u64; 2],
    },
    Remove {
        items: Vec<[u64; 3]>,
    },
    SetBrush {
        changes: Vec<[u64; 3]>,
    },
    Replace {
        #[serde(default)]
        removed: Vec<[u64; 3]>,
        #[serde(default)]
        added: Vec<[u64; 3]>,
    },
    SetGuide {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<GuideDef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<GuideDef>,
    },
}

fn stroke_bytes(s: &Stroke) -> usize {
    s.points.len() * POINT_BYTES
}

/// Where the kept history starts: `undo[undo_from..]` and `redo[redo_from..]`.
///
/// The budget is the sketch's peak size anywhere in its history (at least [`MIN_BUDGET`]),
/// not its current size: after Clear the sketch is empty, but undoing the Clear must still
/// work after reopening. So history can hold as much as the sketch ever did, and the cap
/// only bites on long draw-and-delete cycles. The newest undo step and the nearest redo
/// step are always kept; beyond those, the oldest undo steps go first, then the farthest
/// redo steps.
pub(crate) fn kept(undo: &[Op], redo: &[Op], live: &[Stroke]) -> (usize, usize) {
    let live_ids: HashSet<StrokeId> = live.iter().map(|s| s.id).collect();
    let live_bytes: usize = live.iter().map(stroke_bytes).sum();
    let bytes = |items: &[(usize, Stroke)]| items.iter().map(|(_, s)| stroke_bytes(s) as i64).sum::<i64>();
    // Sketch size at every state the history can reach: undoing from now, redoing from now.
    let mut peak = live_bytes;
    let mut size = live_bytes as i64;
    for op in undo.iter().rev() {
        size += match op {
            Op::Add { stroke, .. } => -(stroke_bytes(stroke) as i64),
            Op::Remove { items } => bytes(items),
            Op::Replace { removed, added } => bytes(removed) - bytes(added),
            _ => 0,
        };
        peak = peak.max(size.max(0) as usize);
    }
    size = live_bytes as i64;
    for op in redo.iter().rev() {
        size += match op {
            Op::Add { stroke, .. } => stroke_bytes(stroke) as i64,
            Op::Remove { items } => -bytes(items),
            Op::Replace { removed, added } => bytes(added) - bytes(removed),
            _ => 0,
        };
        peak = peak.max(size.max(0) as usize);
    }
    let budget = MIN_BUDGET.max(peak);

    let mut counted = HashSet::new();
    let mut cost = |op: &Op| -> usize {
        let pool: usize = op.strokes().filter(|s| !live_ids.contains(&s.id) && counted.insert(s.id)).map(stroke_bytes).sum();
        let guides = match op {
            Op::SetGuide { before, after } => [before, after].iter().filter_map(|g| g.as_ref()).map(|g| guide_bytes(g)).sum(),
            _ => 0,
        };
        pool + guides + 64
    };
    // Always kept: the newest undo step and the nearest redo step.
    let mut used = undo.last().map_or(0, &mut cost) + redo.last().map_or(0, &mut cost);
    let (mut undo_from, mut redo_from) = (undo.len().saturating_sub(1), redo.len().saturating_sub(1));
    // Then, by priority: the rest of redo nearest first, then undo newest first.
    for (i, op) in redo.iter().enumerate().rev().skip(1) {
        used += cost(op);
        if used > budget {
            return (undo_from, i + 1);
        }
        redo_from = i;
    }
    for (i, op) in undo.iter().enumerate().rev().skip(1) {
        used += cost(op);
        if used > budget {
            return (i + 1, redo_from);
        }
        undo_from = i;
    }
    (undo_from, redo_from)
}

fn guide_bytes(g: &airdraw_guide::Guide) -> usize {
    match g {
        airdraw_guide::Guide::Plane { .. } => 24,
        airdraw_guide::Guide::Sweep(s) => (s.profile.len() + s.dirs.len()) * 12,
    }
}

/// Encodes `undo` and `redo`; `brush` maps a snapshot to its index in the file.
pub(crate) fn encode(undo: &[Op], redo: &[Op], live: &[Stroke], brush: &dyn Fn(BrushRef) -> u32) -> Vec<u8> {
    let live_ids: HashSet<StrokeId> = live.iter().map(|s| s.id).collect();
    let mut pool: Vec<Stroke> = Vec::new();
    let mut pooled = HashSet::new();
    for s in undo.iter().chain(redo).flat_map(Op::strokes) {
        if !live_ids.contains(&s.id) && pooled.insert(s.id) {
            pool.push(s.clone());
        }
    }
    let b = |r: BrushRef| brush(r) as u64;
    let def = |op: &Op| match op {
        Op::Add { index, stroke } => OpDef::Add { index: *index as u64, stroke: [stroke.id.0, b(stroke.brush)] },
        Op::Remove { items } => OpDef::Remove { items: items.iter().map(|(i, s)| [*i as u64, s.id.0, b(s.brush)]).collect() },
        Op::SetBrush { changes } => OpDef::SetBrush { changes: changes.iter().map(|&(id, old, new)| [id.0, b(old), b(new)]).collect() },
        Op::Replace { removed, added } => OpDef::Replace {
            removed: removed.iter().map(|(i, s)| [*i as u64, s.id.0, b(s.brush)]).collect(),
            added: added.iter().map(|(i, s)| [*i as u64, s.id.0, b(s.brush)]).collect(),
        },
        Op::SetGuide { before, after } => {
            OpDef::SetGuide { before: before.as_deref().map(GuideDef::new), after: after.as_deref().map(GuideDef::new) }
        }
    };
    let hist = HistDef { undo: undo.iter().map(def).collect(), redo: redo.iter().map(def).collect() };
    let text = toml::to_string(&hist).expect("history always serializes");
    let mut out = Vec::new();
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(text.as_bytes());
    // Pool strokes' own brush field isn't used (each reference has one).
    out.extend(crate::strokes::encode(&pool, |_| 0));
    out
}

/// Decodes history and checks it fits `live` (the sketch's strokes): every op can be undone
/// and redone without touching a stroke that isn't there. `brushes` is the snapshot count.
pub(crate) fn decode(data: &[u8], live: &[Stroke], brushes: usize) -> Result<(Vec<Op>, Vec<Op>), DocError> {
    let bad = |why: String| DocError::Corrupt(format!("history: {why}"));
    let len = u32::from_le_bytes(data.get(..4).ok_or(DocError::Truncated)?.try_into().unwrap()) as usize;
    let text = data.get(4..).and_then(|d| d.get(..len)).ok_or(DocError::Truncated)?;
    let text = std::str::from_utf8(text).map_err(|_| bad("not UTF-8".into()))?;
    let hist: HistDef = toml::from_str(text).map_err(|e| bad(e.to_string()))?;
    let pool = crate::strokes::decode(&data[4 + len..])?;

    let mut points: HashMap<StrokeId, &Stroke> = live.iter().map(|s| (s.id, s)).collect();
    for s in &pool {
        if points.insert(s.id, s).is_some() {
            return Err(bad(format!("stroke {} stored twice", s.id.0)));
        }
    }
    let brush = |n: u64| (n < brushes as u64).then_some(BrushRef(n as u32)).ok_or_else(|| bad(format!("missing brush {n}")));
    let stroke = |id: u64, b: u64| -> Result<Stroke, DocError> {
        let s = points.get(&StrokeId(id)).ok_or_else(|| bad(format!("missing stroke {id}")))?;
        Ok(Stroke { brush: brush(b)?, ..(*s).clone() })
    };
    let items = |items: &[[u64; 3]], what: &str| -> Result<Vec<(usize, Stroke)>, DocError> {
        if !items.windows(2).all(|w| w[0][0] < w[1][0]) {
            return Err(bad(format!("{what} needs ascending indices")));
        }
        items.iter().map(|&[i, id, b]| Ok((i as usize, stroke(id, b)?))).collect()
    };
    let op = |d: OpDef| -> Result<Op, DocError> {
        Ok(match d {
            OpDef::Add { index, stroke: [id, b] } => Op::Add { index: index as usize, stroke: stroke(id, b)? },
            OpDef::Remove { items: list } => {
                if list.is_empty() {
                    return Err(bad("remove of nothing".into()));
                }
                Op::Remove { items: items(&list, "remove")? }
            }
            OpDef::Replace { removed, added } => {
                if removed.is_empty() && added.is_empty() {
                    return Err(bad("replace of nothing".into()));
                }
                Op::Replace { removed: items(&removed, "replace")?, added: items(&added, "replace")? }
            }
            OpDef::SetBrush { changes } => Op::SetBrush {
                changes: changes
                    .iter()
                    .map(|&[id, old, new]| Ok((StrokeId(id), brush(old)?, brush(new)?)))
                    .collect::<Result<_, DocError>>()?,
            },
            OpDef::SetGuide { before, after } => Op::SetGuide {
                before: before.map(GuideDef::into_guide).transpose()?.map(std::sync::Arc::new),
                after: after.map(GuideDef::into_guide).transpose()?.map(std::sync::Arc::new),
            },
        })
    };
    let undo: Vec<Op> = hist.undo.into_iter().map(op).collect::<Result<_, _>>()?;
    let redo: Vec<Op> = hist.redo.into_iter().map(op).collect::<Result<_, _>>()?;
    if undo.len() > crate::document::MAX_UNDO || redo.len() > crate::document::MAX_UNDO {
        return Err(bad("too many steps".into()));
    }
    check(&undo, &redo, live).map_err(bad)?;
    Ok((undo, redo))
}

/// Replays the history on stroke ids: undoing every undo step from the sketch as saved, and
/// redoing every redo step, must only ever touch strokes that are where the ops expect.
/// (Undo then redo of one step returns to the same state, so this covers any order.)
/// Membership is a hash set, so undoing a Clear of a huge sketch stays linear.
fn check(undo: &[Op], redo: &[Op], live: &[Stroke]) -> Result<(), String> {
    let start: Vec<StrokeId> = live.iter().map(|s| s.id).collect();
    let mut st = Ids { has: start.iter().copied().collect(), ids: start.clone() };
    for op in undo.iter().rev() {
        match op {
            // Undoing returns to the state right after the op, so an added stroke must be
            // exactly where it was added (or redo would put it back elsewhere).
            Op::Add { index, stroke } => st.take(&[(*index, stroke.clone())], "undo add")?,
            Op::Remove { items } => st.put(items, "undo remove")?,
            Op::Replace { removed, added } => {
                st.take(added, "undo replace")?;
                st.put(removed, "undo replace")?;
            }
            Op::SetBrush { changes } => present(&st.has, changes.iter().map(|c| c.0), "undo re-brush")?,
            Op::SetGuide { .. } => {}
        }
    }
    let mut st = Ids { has: start.iter().copied().collect(), ids: start };
    for op in redo.iter().rev() {
        match op {
            Op::Add { index, stroke } => st.put(&[(*index, stroke.clone())], "redo add")?,
            Op::Remove { items } => st.take(items, "redo remove")?,
            Op::Replace { removed, added } => {
                st.take(removed, "redo replace")?;
                st.put(added, "redo replace")?;
            }
            Op::SetBrush { changes } => present(&st.has, changes.iter().map(|c| c.0), "redo re-brush")?,
            Op::SetGuide { .. } => {}
        }
    }
    Ok(())
}

/// Stroke ids in sketch order, replayed by [`check`].
struct Ids {
    ids: Vec<StrokeId>,
    has: HashSet<StrokeId>,
}

impl Ids {
    /// Strokes at ascending indices must be exactly there; removed from the back.
    fn take(&mut self, items: &[(usize, Stroke)], what: &str) -> Result<(), String> {
        for (i, s) in items.iter().rev() {
            if self.ids.get(*i) != Some(&s.id) {
                return Err(format!("{what}: stroke {} isn't at {i}", s.id.0));
            }
            self.ids.remove(*i);
            self.has.remove(&s.id);
        }
        Ok(())
    }

    /// Strokes go in at ascending indices, each one new to the sketch.
    fn put(&mut self, items: &[(usize, Stroke)], what: &str) -> Result<(), String> {
        for (i, s) in items {
            if *i > self.ids.len() || !self.has.insert(s.id) {
                return Err(format!("{what}: can't put stroke {} at {i}", s.id.0));
            }
            self.ids.insert(*i, s.id);
        }
        Ok(())
    }
}

fn present(has: &HashSet<StrokeId>, wanted: impl Iterator<Item = StrokeId>, what: &str) -> Result<(), String> {
    for id in wanted {
        if !has.contains(&id) {
            return Err(format!("{what}: stroke {} isn't there", id.0));
        }
    }
    Ok(())
}
