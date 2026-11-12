//! Periodic drape diagnostics for a host that cannot attach a profiler.
//!
//! A drape that waits for its turn shows an ancestor's texture or its own older content, which
//! on screen is indistinguishable from imagery that never loaded. Every [`WINDOW_FRAMES`]
//! frames of a view this logs what its drapes asked for and what the frame budget gave them,
//! so a host sees whether drapes wait, which ones are drawn again and again, and whether the
//! set of draped tiles or the paint zoom keeps changing under them.

use rustc_hash::FxHashMap;

use crate::{
    coords::WorldTileCoords,
    render::eye_covering::{CurrentViewSlot, ViewSlotId},
    tcs::world::World,
    terrain::{drape_cache::DrapeState, drape_targets::TargetSpec},
};

/// Frames of one view between two log lines.
const WINDOW_FRAMES: u32 = 90;

/// What one frame's drapes asked for and got.
pub(super) struct FrameDrapes<'a> {
    pub(super) specs: &'a [TargetSpec],
    pub(super) states: &'a [DrapeState],
    pub(super) ready: &'a [bool],
    /// Drapes prepared ahead of the view, which no surface shows yet.
    pub(super) prefetch: &'a [bool],
    pub(super) redraw: &'a [bool],
    pub(super) sources: &'a [Option<WorldTileCoords>],
    /// Drapes the frame may draw, and the count past which none is drawn.
    pub(super) budget: (usize, usize),
    pub(super) paint_zoom: f64,
}

#[derive(Default)]
struct Window {
    frames: u32,
    targets: usize,
    unready: usize,
    new: usize,
    changed: usize,
    withheld: usize,
    redrawn: usize,
    waiting: usize,
    standing_in: usize,
    budget: usize,
    target_set_changes: u32,
    paint_zoom_changes: u32,
    last_targets: Vec<WorldTileCoords>,
    last_paint_zoom: Option<u64>,
    redraws: FxHashMap<WorldTileCoords, u32>,
}

/// The windows of every view drawing through the map.
#[derive(Default)]
pub(crate) struct DrapeActivity {
    views: FxHashMap<Option<ViewSlotId>, Window>,
}

/// How many of the drapes each view showed in its last frame were not yet drawn from all of
/// their content; the map's own view is `None`. A host waits for a view's image to be final
/// on this, since the drapes a frame draws are capped and the rest show older content or an
/// ancestor meanwhile.
#[derive(Default)]
pub(crate) struct PendingDrapes(FxHashMap<Option<ViewSlotId>, usize>);

impl PendingDrapes {
    #[cfg(feature = "headless")]
    pub(crate) fn of(&self, view: Option<ViewSlotId>) -> usize {
        self.0.get(&view).copied().unwrap_or(0)
    }

    #[cfg(feature = "headless")]
    pub(crate) fn forget(&mut self, view: Option<ViewSlotId>) {
        self.0.remove(&view);
    }
}

impl FrameDrapes<'_> {
    /// Shown drapes whose content is still loading, or that wait for a later frame to be
    /// drawn. A drape withheld for want of a texture is not counted: its tile keeps an
    /// ancestor's drape for as long as the view's allowance stays spent.
    fn pending(&self) -> usize {
        (0..self.specs.len())
            .filter(|index| !self.prefetch.get(*index).copied().unwrap_or(false))
            .filter(|index| {
                let unready = !self.ready.get(*index).copied().unwrap_or(false);
                let drawn = self.redraw.get(*index).copied().unwrap_or(false);
                let waiting = matches!(
                    self.states.get(*index),
                    Some(DrapeState::New | DrapeState::Changed)
                ) && !drawn;
                unready || waiting
            })
            .count()
    }
}

/// Counts a frame's drapes and logs the window when it is full.
pub(super) fn record(world: &mut World, frame: &FrameDrapes<'_>) {
    let view = world
        .resources
        .get::<CurrentViewSlot>()
        .and_then(|slot| slot.0);
    let cost = world
        .resources
        .get::<crate::terrain::drape_timing::DrapeCost>()
        .copied()
        .unwrap_or_default();
    world
        .resources
        .get_or_init_mut::<PendingDrapes>()
        .0
        .insert(view, frame.pending());
    let activity = world.resources.get_or_init_mut::<DrapeActivity>();
    let window = activity.views.entry(view).or_default();
    window.count(frame);
    if window.frames < WINDOW_FRAMES {
        return;
    }
    let most_redrawn = window
        .redraws
        .iter()
        .max_by_key(|(coords, count)| (**count, u8::from(coords.z)))
        .map_or_else(String::new, |(coords, count)| format!("{coords} x{count}"));
    tracing::info!(
        ?view,
        frames = window.frames,
        targets = window.targets,
        unready = window.unready,
        new = window.new,
        changed = window.changed,
        withheld = window.withheld,
        redrawn = window.redrawn,
        waiting = window.waiting,
        standing_in = window.standing_in,
        budget = window.budget,
        tiles_redrawn = window.redraws.len(),
        %most_redrawn,
        target_set_changes = window.target_set_changes,
        paint_zoom_changes = window.paint_zoom_changes,
        drape_cost_us = cost.per_drape().as_micros(),
        cost_measured = cost.last_frame().is_some(),
        "drape activity, summed over the frames"
    );
    *window = Window {
        last_targets: std::mem::take(&mut window.last_targets),
        last_paint_zoom: window.last_paint_zoom,
        ..Window::default()
    };
}

impl Window {
    fn count(&mut self, frame: &FrameDrapes<'_>) {
        self.frames += 1;
        self.targets += frame.specs.len();
        self.budget += frame.budget.0;
        let targets: Vec<WorldTileCoords> = frame.specs.iter().map(|spec| spec.coords).collect();
        if self.frames > 1 || !self.last_targets.is_empty() {
            self.target_set_changes += u32::from(targets != self.last_targets);
        }
        self.last_targets = targets;
        let paint_zoom = frame.paint_zoom.to_bits();
        self.paint_zoom_changes +=
            u32::from(self.last_paint_zoom.is_some_and(|last| last != paint_zoom));
        self.last_paint_zoom = Some(paint_zoom);
        for (index, spec) in frame.specs.iter().enumerate() {
            let state = frame.states.get(index).copied();
            let drawn = frame.redraw.get(index).copied().unwrap_or(false);
            self.unready += usize::from(!frame.ready.get(index).copied().unwrap_or(false));
            self.new += usize::from(state == Some(DrapeState::New));
            self.changed += usize::from(state == Some(DrapeState::Changed));
            self.withheld += usize::from(state == Some(DrapeState::Withheld));
            self.redrawn += usize::from(drawn);
            self.waiting +=
                usize::from(matches!(state, Some(DrapeState::New | DrapeState::Changed)) && !drawn);
            self.standing_in +=
                usize::from(frame.sources.get(index).copied().flatten() != Some(spec.coords));
            if drawn {
                *self.redraws.entry(spec.coords).or_default() += 1;
            }
        }
    }
}
