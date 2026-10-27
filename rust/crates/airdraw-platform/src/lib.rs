//! The platform contract. Each OS backend (Swift on iOS, Kotlin on Android, Godot on desktop)
//! implements these traits; the rest of the app only talks to [`Platform`].
//!
//! Anything a backend doesn't provide falls back to a default, so a new platform runs on day
//! one and gains native pieces one at a time. Godot-based fallbacks live in `airdraw-godot`,
//! the only crate allowed to depend on Godot.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub use airdraw_input::{DeviceCaps, PenEvent};

/// Source of normalized pen/touch/mouse events.
pub trait InputSource: Send {
    fn caps(&self) -> DeviceCaps;
    /// Moves all events received since the last call into `out`, oldest first.
    fn poll(&mut self, out: &mut Vec<PenEvent>);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HapticKind {
    /// Light tick, e.g. snapping to a guide.
    Snap,
    /// Confirmation, e.g. a squeeze or double-tap was recognized.
    Action,
    Warning,
}

pub trait Haptics: Send {
    fn play(&self, kind: HapticKind);
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileFilter {
    pub description: String,
    /// Extensions without the dot, e.g. ["airdraw"].
    pub extensions: Vec<String>,
}

/// Called with the chosen path, or `None` if the user cancelled.
pub type PickCallback = Box<dyn FnOnce(Option<PathBuf>) + Send>;

/// File locations and pickers. Pickers are asynchronous because mobile OSes show them
/// as separate system screens.
pub trait Storage: Send {
    /// Folder where the app keeps its documents.
    fn documents_dir(&self) -> PathBuf;
    /// Folder for user-installed brushes and plugins.
    fn user_data_dir(&self) -> PathBuf;
    fn pick_open(&self, filter: FileFilter, done: PickCallback);
    fn pick_save(&self, suggested_name: String, filter: FileFilter, done: PickCallback);
}

/// Hands a finished file to the OS share sheet (or a save dialog on desktop).
pub trait Share: Send {
    fn share_file(&self, path: PathBuf, mime_type: String);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceClass {
    Phone,
    Tablet,
    Desktop,
}

pub trait DisplayInfo: Send {
    fn refresh_rate_hz(&self) -> f32;
    /// Physical pixels per logical point.
    fn scale(&self) -> f32;
    fn device_class(&self) -> DeviceClass;
}

/// All platform services. Built once at startup by the active backend.
pub struct Platform {
    pub input: Box<dyn InputSource>,
    pub haptics: Box<dyn Haptics>,
    pub storage: Box<dyn Storage>,
    pub share: Box<dyn Share>,
    pub display: Box<dyn DisplayInfo>,
}

impl Platform {
    /// Starts from null implementations; backends replace the services they provide.
    pub fn builder(data_root: PathBuf) -> PlatformBuilder {
        PlatformBuilder {
            platform: Platform {
                input: Box::new(QueueInput::new(DeviceCaps::MOUSE)),
                haptics: Box::new(NoHaptics),
                storage: Box::new(DirStorage::new(data_root)),
                share: Box::new(NoShare),
                display: Box::new(FixedDisplay::DESKTOP),
            },
        }
    }
}

pub struct PlatformBuilder {
    platform: Platform,
}

impl PlatformBuilder {
    pub fn input(mut self, v: impl InputSource + 'static) -> Self {
        self.platform.input = Box::new(v);
        self
    }
    pub fn haptics(mut self, v: impl Haptics + 'static) -> Self {
        self.platform.haptics = Box::new(v);
        self
    }
    pub fn storage(mut self, v: impl Storage + 'static) -> Self {
        self.platform.storage = Box::new(v);
        self
    }
    pub fn share(mut self, v: impl Share + 'static) -> Self {
        self.platform.share = Box::new(v);
        self
    }
    pub fn display(mut self, v: impl DisplayInfo + 'static) -> Self {
        self.platform.display = Box::new(v);
        self
    }
    pub fn build(self) -> Platform {
        self.platform
    }
}

// ---- Default and test implementations -------------------------------------------------

/// Input fed by code through a [`QueueSender`]: the Godot fallback pushes converted Godot
/// events into it, and tests push recorded strokes to replay them. Native backends will use a
/// lock-free queue instead of this mutex-backed one.
pub struct QueueInput {
    caps: DeviceCaps,
    queue: Arc<Mutex<VecDeque<PenEvent>>>,
}

/// Cloneable handle that feeds a [`QueueInput`].
#[derive(Clone)]
pub struct QueueSender(Arc<Mutex<VecDeque<PenEvent>>>);

impl QueueSender {
    pub fn push(&self, e: PenEvent) {
        lock(&self.0).push_back(e);
    }
}

impl QueueInput {
    pub fn new(caps: DeviceCaps) -> Self {
        Self { caps, queue: Arc::default() }
    }
    pub fn sender(&self) -> QueueSender {
        QueueSender(self.queue.clone())
    }
}

impl InputSource for QueueInput {
    fn caps(&self) -> DeviceCaps {
        self.caps
    }
    fn poll(&mut self, out: &mut Vec<PenEvent>) {
        out.extend(lock(&self.queue).drain(..));
    }
}

/// A panic while holding the lock can't leave a queue of plain events inconsistent.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Several input sources as one, e.g. the native touch backend plus Godot events for
/// keyboard, mouse and trackpad. Polls in order; capabilities are combined.
pub struct MergedInput(pub Vec<Box<dyn InputSource>>);

impl InputSource for MergedInput {
    fn caps(&self) -> DeviceCaps {
        self.0.iter().fold(DeviceCaps::MOUSE, |acc, s| acc.union(s.caps()))
    }
    fn poll(&mut self, out: &mut Vec<PenEvent>) {
        for s in &mut self.0 {
            s.poll(out);
        }
    }
}

pub struct NoHaptics;

impl Haptics for NoHaptics {
    fn play(&self, _: HapticKind) {}
}

/// Plain folders under a root, with pickers that always cancel. Enough for tests and as a
/// placeholder until a backend provides real pickers.
pub struct DirStorage {
    root: PathBuf,
}

impl DirStorage {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl Storage for DirStorage {
    fn documents_dir(&self) -> PathBuf {
        self.root.join("documents")
    }
    fn user_data_dir(&self) -> PathBuf {
        self.root.join("user")
    }
    fn pick_open(&self, _: FileFilter, done: PickCallback) {
        done(None)
    }
    fn pick_save(&self, _: String, _: FileFilter, done: PickCallback) {
        done(None)
    }
}

pub struct NoShare;

impl Share for NoShare {
    fn share_file(&self, _: PathBuf, _: String) {}
}

pub struct FixedDisplay {
    pub refresh_rate_hz: f32,
    pub scale: f32,
    pub device_class: DeviceClass,
}

impl FixedDisplay {
    pub const DESKTOP: Self = Self { refresh_rate_hz: 60.0, scale: 1.0, device_class: DeviceClass::Desktop };
}

impl DisplayInfo for FixedDisplay {
    fn refresh_rate_hz(&self) -> f32 {
        self.refresh_rate_hz
    }
    fn scale(&self) -> f32 {
        self.scale
    }
    fn device_class(&self) -> DeviceClass {
        self.device_class
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use airdraw_input::{PenSample, PointerId, PointerTool};

    #[test]
    fn queue_input_replays_in_order() {
        let mut input = QueueInput::new(DeviceCaps::MOUSE);
        let tx = input.sender();
        let id = PointerId(1);
        let s = PenSample::at(Default::default(), 0.0);
        tx.push(PenEvent::Down { id, tool: PointerTool::Pen, sample: s, modifiers: Default::default() });
        tx.push(PenEvent::Up { id, sample: s });
        let mut out = Vec::new();
        input.poll(&mut out);
        assert!(matches!(out[0], PenEvent::Down { .. }));
        assert!(matches!(out[1], PenEvent::Up { .. }));
        out.clear();
        input.poll(&mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn merged_input_polls_all_and_unions_caps() {
        let a = QueueInput::new(DeviceCaps { pressure: true, ..DeviceCaps::MOUSE });
        let b = QueueInput::new(DeviceCaps { hover: true, ..DeviceCaps::MOUSE });
        let (ta, tb) = (a.sender(), b.sender());
        let mut m = MergedInput(vec![Box::new(a), Box::new(b)]);
        ta.push(PenEvent::HoverExit);
        tb.push(PenEvent::HoverExit);
        let mut out = Vec::new();
        m.poll(&mut out);
        assert_eq!(out.len(), 2);
        assert!(m.caps().pressure && m.caps().hover);
    }

    #[test]
    fn builder_replaces_only_given_services() {
        let p = Platform::builder("/tmp/airdraw".into())
            .display(FixedDisplay { refresh_rate_hz: 120.0, scale: 2.0, device_class: DeviceClass::Tablet })
            .build();
        assert_eq!(p.display.device_class(), DeviceClass::Tablet);
        assert_eq!(p.storage.documents_dir(), PathBuf::from("/tmp/airdraw/documents"));
    }

    #[test]
    fn default_picker_cancels() {
        let p = Platform::builder("/tmp/airdraw".into()).build();
        let got = Arc::new(Mutex::new(Some(PathBuf::new())));
        let g = got.clone();
        p.storage.pick_open(FileFilter::default(), Box::new(move |r| *g.lock().unwrap() = r));
        assert_eq!(*got.lock().unwrap(), None);
    }
}
