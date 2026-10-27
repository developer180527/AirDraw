use std::collections::BTreeMap;
use std::sync::Arc;

use crate::stage::{Dynamics, Filter, Geometry, Material};
use crate::stages::{dynamics, filters, geometry, material};

/// Named stage implementations that brush files refer to by `type`.
#[derive(Default, Clone)]
pub struct StageRegistry {
    pub(crate) filters: BTreeMap<String, Arc<dyn Filter>>,
    pub(crate) dynamics: BTreeMap<String, Arc<dyn Dynamics>>,
    pub(crate) geometry: BTreeMap<String, Arc<dyn Geometry>>,
    pub(crate) materials: BTreeMap<String, Arc<dyn Material>>,
}

impl StageRegistry {
    pub fn with_builtins() -> Self {
        let mut r = Self::default();
        r.register_filter("resample", filters::Resample);
        r.register_filter("exponential", filters::Exponential);
        r.register_filter("one_euro", filters::OneEuro);
        r.register_filter("spline", filters::Spline);
        r.register_dynamics("width", dynamics::Width);
        r.register_dynamics("taper", dynamics::Taper);
        r.register_geometry("tube", geometry::Tube);
        r.register_geometry("ribbon", geometry::Ribbon);
        r.register_geometry("dots", geometry::Dots);
        r.register_material("solid", material::Solid);
        r.register_material("flat", material::Flat);
        r
    }

    pub fn register_filter(&mut self, name: &str, stage: impl Filter + 'static) {
        self.filters.insert(name.into(), Arc::new(stage));
    }
    pub fn register_dynamics(&mut self, name: &str, stage: impl Dynamics + 'static) {
        self.dynamics.insert(name.into(), Arc::new(stage));
    }
    pub fn register_geometry(&mut self, name: &str, stage: impl Geometry + 'static) {
        self.geometry.insert(name.into(), Arc::new(stage));
    }
    pub fn register_material(&mut self, name: &str, stage: impl Material + 'static) {
        self.materials.insert(name.into(), Arc::new(stage));
    }

    /// Stage names per slot, for the brush editor's pickers.
    pub fn names(&self) -> StageNames {
        StageNames {
            filters: self.filters.keys().cloned().collect(),
            dynamics: self.dynamics.keys().cloned().collect(),
            geometry: self.geometry.keys().cloned().collect(),
            materials: self.materials.keys().cloned().collect(),
        }
    }
}

/// The four places a stage can go in a brush.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Filter,
    Dynamics,
    Geometry,
    Material,
}

impl StageRegistry {
    /// Parameter schema of a registered stage, for validation and generated editor UI.
    pub fn schema(&self, slot: Slot, kind: &str) -> Option<Vec<crate::ParamSpec>> {
        match slot {
            Slot::Filter => self.filters.get(kind).map(|s| s.schema()),
            Slot::Dynamics => self.dynamics.get(kind).map(|s| s.schema()),
            Slot::Geometry => self.geometry.get(kind).map(|s| s.schema()),
            Slot::Material => self.materials.get(kind).map(|s| s.schema()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct StageNames {
    pub filters: Vec<String>,
    pub dynamics: Vec<String>,
    pub geometry: Vec<String>,
    pub materials: Vec<String>,
}
