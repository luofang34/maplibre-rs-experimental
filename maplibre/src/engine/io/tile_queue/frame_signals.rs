//! Core frame readiness signals and per-frame statistics shared by tile loading and rendering.

use std::{sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};

use crate::{coords::WorldTileCoords, io::apc::MessageTag, tcs::world::World};

/// Request families whose completion controls a retry deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RequestKind {
    /// Decoded map imagery.
    Raster,
    /// Elevation data used for terrain and shading.
    Dem,
    /// Vector geometry and its associated symbol processing.
    Vector,
}

impl MessageTag for RequestKind {
    fn dyn_clone(&self) -> Box<dyn MessageTag> {
        Box::new(*self)
    }
}

impl RequestKind {
    /// Identifies final request results independently from pixel payloads.
    pub fn message_tag(self) -> &'static dyn MessageTag {
        match self {
            Self::Raster => &Self::Raster,
            Self::Dem => &Self::Dem,
            Self::Vector => &Self::Vector,
        }
    }
}

/// Which eye of a frame the schedule is running for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EyeInFrame {
    /// Position of the eye in the frame; the first eye selects the tiles.
    pub index: usize,
    /// The frame, so a selection is never reused across frames.
    pub frame: u64,
}

impl EyeInFrame {
    /// Whether this eye must reuse the content selected for an earlier eye of the frame.
    pub(crate) fn reuses_content(world: &World) -> bool {
        world
            .resources
            .get::<Self>()
            .is_some_and(|eye| eye.index > 0)
    }
}

/// A resource that finished loading, as a host learns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceReady {
    /// A tile request of `kind` finished; `loaded` is false when it failed and will be retried.
    Tile {
        /// The tile.
        coords: WorldTileCoords,
        /// What was requested for it.
        kind: RequestKind,
        /// Whether the request delivered its data.
        loaded: bool,
    },
    /// A tile's glyphs and icons reached the GPU, so its labels can draw.
    SymbolAtlas {
        /// The tile.
        coords: WorldTileCoords,
    },
}

/// What one frame cost.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameStats {
    /// The frame's number, counting from one.
    pub frame: u64,
    /// The host's frame this belongs to: one per frame, or one per XR frame whatever its eyes,
    /// which a frame timeline records the frame under.
    pub host_frame: u64,
    /// CPU time of each schedule stage, in the order they ran.
    pub stages: Vec<(Arc<str>, Duration)>,
    /// GPU time of the latest main pass whose timestamps came back, usually the previous
    /// frame's; absent when the device cannot time passes or none came back this frame.
    pub gpu: Option<Duration>,
    /// Bytes written to GPU buffers and textures.
    pub upload_bytes: u64,
    /// Terrain drapes a view shows that were drawn again.
    pub drape_redraws: u32,
    /// Terrain drapes prepared ahead of the view, which no surface shows yet.
    pub prefetch_drapes: u32,
    /// Draw calls recorded.
    pub draws: u32,
}

impl FrameStats {
    /// The CPU time of all stages together.
    pub fn cpu(&self) -> Duration {
        self.stages.iter().map(|(_, spent)| *spent).sum()
    }

    /// Adds what the views drawn before this one in the same host frame cost, so a host that
    /// draws several views in one call reads the cost of the call and not of its last view.
    /// Draw calls are already counted across views, and the GPU time stays that of the
    /// latest pass measured.
    #[cfg(feature = "headless")]
    pub(crate) fn add_earlier_views(&mut self, earlier: &FrameStats) {
        for (stage, spent) in &earlier.stages {
            match self.stages.iter_mut().find(|(name, _)| name == stage) {
                Some((_, total)) => *total += *spent,
                None => self.stages.push((stage.clone(), *spent)),
            }
        }
        self.gpu = self.gpu.or(earlier.gpu);
        self.upload_bytes = self.upload_bytes.saturating_add(earlier.upload_bytes);
        self.drape_redraws = self.drape_redraws.saturating_add(earlier.drape_redraws);
        self.prefetch_drapes = self.prefetch_drapes.saturating_add(earlier.prefetch_drapes);
    }
}

/// Why a map asks for another frame, so a host that never sees it settle can tell what keeps
/// it drawing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RedrawReasons {
    /// The map never drew, or something changed or still animates since the last frame.
    pub changed: bool,
    /// A tile request is in flight, or a retry of one is due.
    pub loading: bool,
    /// Provided images a tile waits for are still being made.
    pub awaiting_images: bool,
    /// The camera differs from the one the last frame drew.
    pub camera_moved: bool,
    /// Worker results wait to be applied; only a host that owns the worker transport sees it.
    pub worker_results: bool,
}

impl RedrawReasons {
    /// Whether any reason asks for a frame.
    pub fn any(&self) -> bool {
        self.changed
            || self.loading
            || self.awaiting_images
            || self.camera_moved
            || self.worker_results
    }
}

/// The signals a frame leaves for its host.
#[derive(Default)]
pub struct FrameSignals {
    pub(crate) current: FrameStats,
    pub(crate) last: FrameStats,
    pub(crate) frame: u64,
    pub(crate) events: Vec<ResourceReady>,
    pub(crate) dirty: bool,
    pub(crate) animating: bool,
    pub(crate) host_frame: u64,
    /// The camera each eye drew last, by eye index; a map without eyes is eye 0.
    pub(crate) cameras: Vec<[f64; 24]>,
    /// Counts the frames whose first eye drew a camera other than the frame before.
    pub(crate) camera_revision: u64,
}

impl FrameSignals {
    fn record_resource_ready(&mut self, ready: ResourceReady) {
        self.dirty = true;
        self.events.push(ready);
    }

    fn add_drape_counts(&mut self, redraws: usize, prefetched: usize) {
        saturating_add_usize(&mut self.current.drape_redraws, redraws);
        saturating_add_usize(&mut self.current.prefetch_drapes, prefetched);
    }
}

#[inline]
fn saturating_add_usize(dst: &mut u32, add: usize) {
    *dst = dst.saturating_add(u32::try_from(add).unwrap_or(u32::MAX));
}

#[inline]
fn last_host_frame(world: &World) -> u64 {
    world
        .resources
        .get::<FrameSignals>()
        .map_or(0, |signals| signals.host_frame)
}

#[inline]
fn is_later_eye(world: &World) -> bool {
    world
        .resources
        .get::<EyeInFrame>()
        .is_some_and(|eye| eye.index > 0)
}

pub(crate) fn signals(world: &mut World) -> &mut FrameSignals {
    world.resources.get_or_init_mut::<FrameSignals>()
}

/// The map changed in a way the next frame shows.
pub(crate) fn mark_dirty(world: &mut World) {
    signals(world).dirty = true;
}

/// Something on screen is still moving, so the frame after this one differs from it.
pub(crate) fn keep_animating(world: &mut World) {
    signals(world).animating = true;
}

/// Whether the last finished pass left the map marked dirty for a follow-up frame.
#[cfg(feature = "headless")]
pub(crate) fn is_dirty(world: &World) -> bool {
    world
        .resources
        .get::<FrameSignals>()
        .is_some_and(|signals| signals.dirty)
}

/// Folds the cost of the views drawn earlier in a host frame of several views into the
/// statistics of the view that just finished, and returns the frame's cost so far.
#[cfg(feature = "headless")]
pub(crate) fn total_frame_stats(world: &mut World, earlier: Option<&FrameStats>) -> FrameStats {
    let last = &mut signals(world).last;
    if let Some(earlier) = earlier {
        last.add_earlier_views(earlier);
    }
    last.clone()
}

/// A resource finished loading; the map is dirty and the host is told.
pub(crate) fn resource_ready(world: &mut World, ready: ResourceReady) {
    signals(world).record_resource_ready(ready);
}

/// The eye being drawn: its index in an XR frame, 0 otherwise.
pub(crate) fn eye_index(world: &World) -> usize {
    world
        .resources
        .get::<EyeInFrame>()
        .map_or(0, |eye| eye.index)
}

/// Counts terrain drapes drawn again this frame.
pub(crate) fn count_drape_redraws(world: &mut World, redraws: usize, prefetched: usize) {
    signals(world).add_drape_counts(redraws, prefetched);
}

/// The host frame the frame being drawn belongs to: a new one, unless it is a later eye of
/// an XR frame.
pub(crate) fn host_frame_in_progress(world: &World) -> u64 {
    let last = last_host_frame(world);
    if is_later_eye(world) {
        last
    } else {
        last.wrapping_add(1)
    }
}
