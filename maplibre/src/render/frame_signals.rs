//! What a host needs to render on demand: whether the map needs another frame, which resources
//! became ready, and what each frame cost.
//!
//! The map is dirty after data arrives or the style changes, stays animating while labels fade
//! or terrain drapes wait for their redraw budget, needs a frame while tile requests are in
//! flight, and needs one after the camera moves. A frame's statistics are gathered as its stages
//! run and closed at the end of the schedule.

use std::time::Duration;

use crate::{
    context::MapContext, coords::WorldTileCoords, io::tile_retry::RequestKind, tcs::world::World,
};

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
    pub stages: Vec<(String, Duration)>,
    /// GPU time of the latest main pass whose timestamps came back, usually the previous
    /// frame's; absent when the device cannot time passes or none came back this frame.
    pub gpu: Option<Duration>,
    /// Bytes written to GPU buffers and textures.
    pub upload_bytes: u64,
    /// Terrain drapes drawn again.
    pub drape_redraws: u32,
    /// Draw calls recorded.
    pub draws: u32,
}

impl FrameStats {
    /// The CPU time of all stages together.
    pub fn cpu(&self) -> Duration {
        self.stages.iter().map(|(_, spent)| *spent).sum()
    }
}

/// The signals a frame leaves for its host.
#[derive(Default)]
pub struct FrameSignals {
    current: FrameStats,
    last: FrameStats,
    frame: u64,
    events: Vec<ResourceReady>,
    dirty: bool,
    animating: bool,
    host_frame: u64,
    /// The camera each eye drew last, by eye index; a map without eyes is eye 0.
    cameras: Vec<[f64; 24]>,
}

fn signals(world: &mut World) -> &mut FrameSignals {
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

/// A resource finished loading; the map is dirty and the host is told.
pub(crate) fn resource_ready(world: &mut World, ready: ResourceReady) {
    let signals = signals(world);
    signals.dirty = true;
    signals.events.push(ready);
}

/// Whether the camera differs from the one the last frame drew, as while a gesture runs.
pub(crate) fn camera_moved(
    world: &World,
    view_state: &crate::render::view_state::ViewState,
) -> bool {
    world
        .resources
        .get::<FrameSignals>()
        .and_then(|signals| signals.cameras.get(eye_index(world)))
        .is_some_and(|drawn| *drawn != camera_of(view_state))
}

/// The eye being drawn: its index in an XR frame, 0 otherwise.
fn eye_index(world: &World) -> usize {
    world
        .resources
        .get::<crate::render::eye_covering::EyeInFrame>()
        .map_or(0, |eye| eye.index)
}

/// Charges a stage's CPU time to the current frame.
pub(crate) fn record_stage(world: &mut World, stage: String, spent: Duration) {
    signals(world).current.stages.push((stage, spent));
}

/// Counts terrain drapes drawn again this frame.
pub(crate) fn count_drape_redraws(world: &mut World, redraws: usize) {
    let current = &mut signals(world).current;
    current.drape_redraws = current
        .drape_redraws
        .saturating_add(u32::try_from(redraws).unwrap_or(u32::MAX));
}

/// Records the GPU time of the latest measured main pass, which reaches the CPU a frame or more
/// after its own frame.
pub(crate) fn record_gpu_time(world: &mut World, host_frame: u64, spent: Duration) {
    signals(world).current.gpu = Some(spent);
    // The timeline files the time under the frame whose pass it measured.
    if let Some(trace) = world
        .resources
        .get_mut::<crate::render::frame_trace::FrameTraceSlot>()
        .and_then(|slot| slot.0.as_mut())
    {
        trace.record_span(
            host_frame,
            "map",
            crate::render::frame_trace::Clock::Gpu,
            spent,
        );
    }
}

/// The host frame the frame being drawn belongs to: a new one, unless it is a later eye of
/// an XR frame.
pub(crate) fn host_frame_in_progress(world: &World) -> u64 {
    let last = world
        .resources
        .get::<FrameSignals>()
        .map_or(0, |signals| signals.host_frame);
    let later_eye = world
        .resources
        .get::<crate::render::eye_covering::EyeInFrame>()
        .is_some_and(|eye| eye.index > 0);
    if later_eye {
        last
    } else {
        last.wrapping_add(1)
    }
}

fn camera(context: &MapContext) -> [f64; 24] {
    camera_of(&context.view_state)
}

fn camera_of(view_state: &crate::render::view_state::ViewState) -> [f64; 24] {
    let matrix: [[f64; 4]; 4] = view_state.view_projection().0.into();
    let mut flat = [0.0; 24];
    for (column, values) in matrix.iter().enumerate() {
        flat[column * 4..column * 4 + 4].copy_from_slice(values);
    }
    // Over a cap the flat camera stops at the last row of tiles while a free pose moves on.
    if let Some(pose) = view_state.globe_pose() {
        flat[16..19].copy_from_slice(&pose.target);
        flat[19..23].copy_from_slice(&pose.orientation);
        flat[23] = pose.distance_meters;
    }
    flat
}

/// Closes the frame: its uploads and draws are counted, its statistics become the last frame's,
/// and the camera it drew is remembered.
pub(crate) fn finish_frame(context: &mut MapContext) {
    let upload_bytes = context.renderer.queue.take_written_bytes();
    let draws = context
        .world
        .resources
        .get::<crate::render::tracked_pass::RenderStats>()
        .map_or(0, |stats| stats.peek().draws);
    let drawn = camera(context);
    let eye = eye_index(&context.world);
    let host_frame = host_frame_in_progress(&context.world);
    let signals = signals(&mut context.world);
    signals.host_frame = host_frame;
    signals.frame = signals.frame.wrapping_add(1);
    let mut finished = std::mem::take(&mut signals.current);
    finished.frame = signals.frame;
    finished.host_frame = host_frame;
    finished.upload_bytes = upload_bytes;
    finished.draws = draws;
    signals.last = finished;
    signals.dirty = std::mem::take(&mut signals.animating);
    if signals.cameras.len() <= eye {
        signals.cameras.resize(eye + 1, drawn);
    }
    signals.cameras[eye] = drawn;
    if let Some((signals, slot)) = context.world.resources.query_mut::<(
        &mut FrameSignals,
        &mut crate::render::frame_trace::FrameTraceSlot,
    )>() {
        if let Some(trace) = slot.0.as_mut() {
            trace.record_map_frame(&signals.last);
        }
    }
}

impl MapContext {
    /// Whether the map should draw another frame: something changed or still animates since
    /// the last one, tiles are still loading, or the camera moved.
    pub fn needs_redraw(&self) -> bool {
        let loading = crate::io::tile_retry::needs_frame(&self.world);
        let Some(signals) = self.world.resources.get::<FrameSignals>() else {
            return true;
        };
        signals.dirty
            || loading
            || signals.cameras.get(eye_index(&self.world)) != Some(&camera(self))
    }

    /// The statistics of the last finished frame.
    pub fn last_frame_stats(&self) -> FrameStats {
        self.world
            .resources
            .get::<FrameSignals>()
            .map(|signals| signals.last.clone())
            .unwrap_or_default()
    }

    /// The resources that became ready since the last call.
    pub fn take_ready_resources(&mut self) -> Vec<ResourceReady> {
        std::mem::take(&mut signals(&mut self.world).events)
    }
}

#[cfg(test)]
mod tests;
