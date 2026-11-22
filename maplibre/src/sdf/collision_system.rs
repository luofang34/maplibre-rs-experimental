//! Places elevated text and icons on discrete placement epochs, interpolating fades on the GPU.
use std::borrow::Cow;

pub use controller::PlacementConfig;
use controller::{PlacementController, PlacementTriggers};
use rustc_hash::{FxHashMap, FxHashSet};
#[cfg(test)]
use signatures::obstructed_grid;
use signatures::visible_layers;

use super::collision_grid::CollisionGrid;
#[cfg(test)]
use super::placement::canonical_tile;
use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    render::{eventually::Eventually, projection::projection_data_for_view},
    sdf::SymbolBufferPool,
    tcs::system::{System, SystemError, SystemResult},
};

mod active;
mod controller;
mod draw_sort;
mod family;
mod layer_pass;
mod repeat_spacing;
mod rules;
mod signatures;
mod sliced;
mod sync;
mod temporal;
mod upload_diff;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod proptests;
#[cfg(test)]
mod tests;

type ViewSignature = (cgmath::Matrix4<f64>, (f64, f64));

/// Places visible symbols across tiles on throttled epochs and shares placement between eyes.
#[derive(Default)]
pub struct CollisionSystem {
    runs: u32,
    controller: PlacementController,
    active_epoch: Option<sliced::ActiveEpoch>,
    /// View the last committed placement epoch ran for.
    placed_view: Option<ViewSignature>,
    /// View seen on the immediately preceding frame, used to detect single-step XR movement.
    last_frame_view: Option<ViewSignature>,
    last_content_signature: Option<u64>,
    last_terrain_signature: Option<u64>,
    last_external_view: Option<bool>,
    history: temporal::PlacementHistory,
    uploaded:
        FxHashMap<(WorldTileCoords, crate::style::StyleLayerId), upload_diff::CachedBucketMetadata>,
    scratch: upload_diff::MetadataScratchPool,
    draw_sort: draw_sort::DrawSort,
    /// Persistent scratch set of currently visible covering tile coordinates.
    seen: FxHashSet<WorldTileCoords>,
    sig_cache: signatures::SignatureCache,
    grid_cache: CollisionGrid,
    /// Revision of the host's label obstacles the last placement kept clear.
    obstacles: u64,
    initial_boot_loading: bool,
    initial_boot_finished: bool,
    initial_boot_started_at: Option<std::time::Duration>,
}

/// World-level counter recording how many times [`CollisionSystem`] has executed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CollisionStats {
    /// Total non-short-circuited [`CollisionSystem`] ticks on this world.
    pub runs: u32,
}

impl CollisionSystem {
    /// Starts with no placement history or uploaded metadata cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of times placement checks have executed on this system instance.
    pub fn runs(&self) -> u32 {
        self.runs
    }

    fn build_triggers(
        &mut self,
        ctx: &MapContext,
        content_sig: u64,
        terrain_sig: u64,
    ) -> (PlacementTriggers, ViewSignature, bool, u64) {
        let external_view = ctx.view_state.has_external_view();
        let view = (
            ctx.view_state.view_projection().0,
            ctx.view_state.viewport_size(),
        );
        let view_valid = AsRef::<[[f64; 4]; 4]>::as_ref(&view.0)
            .iter()
            .flatten()
            .all(|value| value.is_finite());
        let prev_frame_view = self.last_frame_view;
        let moved = !external_view && view_valid && self.placed_view.as_ref() != Some(&view);
        let moving_this_frame = !external_view
            && view_valid
            && prev_frame_view.is_some()
            && prev_frame_view.as_ref() != Some(&view);
        let external_moved = external_view && view_valid && prev_frame_view.as_ref() != Some(&view);
        if view_valid {
            self.last_frame_view = Some(view);
        }
        let obstacles = ctx
            .world
            .resources
            .get::<super::obstacles::LabelObstacles>()
            .map_or(0, super::obstacles::LabelObstacles::revision);
        let triggers = PlacementTriggers {
            moved,
            moving_this_frame,
            external_view,
            external_moved,
            new_content: self.last_content_signature != Some(content_sig),
            environment_changed: self.last_terrain_signature != Some(terrain_sig)
                || self.last_external_view != Some(external_view),
            obstructed: obstacles != self.obstacles,
        };
        (triggers, view, view_valid, obstacles)
    }

    fn maybe_sort_bearing(
        &mut self,
        ctx: &mut MapContext,
        zoom: f64,
        moving_this_frame: bool,
        caller_immediate: bool,
    ) -> bool {
        let bearing = ctx.view_state.camera().get_bearing().0;
        if moving_this_frame || !self.draw_sort.might_need_sort(bearing) {
            return false;
        }
        let layers = visible_layers(&ctx.world, &ctx.style, zoom, &self.seen);
        let pre_sort_budget = (!caller_immediate && self.controller.last_dt_non_zero())
            .then_some((std::time::Duration::from_micros(400), 3));
        if self
            .draw_sort
            .needs_bearing_sort(&ctx.world, (bearing, zoom), &layers)
        {
            self.draw_sort.follow_bearing(
                &ctx.world,
                &ctx.renderer.queue,
                (bearing, zoom),
                &layers,
                pre_sort_budget,
            )
        } else {
            false
        }
    }

    fn run_skipped_or_deferred(
        &mut self,
        ctx: &mut MapContext,
        now: std::time::Duration,
        in_flight: usize,
        sort_pending: bool,
    ) {
        self.history.touch_active_states(now);
        active::sync_and_prune(
            &mut ctx.world,
            &ctx.renderer,
            &self.history,
            &self.uploaded,
            ctx.view_state.camera().get_pitch().0,
        );
        if self.history.is_fading() || self.controller.is_pending() || sort_pending {
            crate::render::frame_signals::keep_animating(&mut ctx.world);
        } else if self.initial_boot_loading
            && active::can_finish_initial_boot(&ctx.world, in_flight)
        {
            self.initial_boot_finished = true;
            self.initial_boot_loading = false;
        }
    }

    fn try_short_circuit_boot_loading(
        &mut self,
        ctx: &mut MapContext,
        now: std::time::Duration,
        in_flight: usize,
        triggers: &PlacementTriggers,
    ) -> bool {
        if !(self.initial_boot_loading
            && in_flight > 0
            && !triggers.moving_this_frame
            && !triggers.obstructed
            && self.initial_boot_started_at.is_some_and(|start| {
                now.saturating_sub(start) < std::time::Duration::from_millis(250)
            }))
        {
            return false;
        }
        self.controller.advance_clock(now);
        active::sync_and_prune(
            &mut ctx.world,
            &ctx.renderer,
            &self.history,
            &self.uploaded,
            ctx.view_state.camera().get_pitch().0,
        );
        true
    }

    fn try_step_active_epoch(
        &mut self,
        ctx: &mut MapContext,
        triggers: &PlacementTriggers,
        (zoom, now, in_flight): (f64, std::time::Duration, usize),
        (caller_immediate, view_valid, earlier_stage_spent): (bool, bool, std::time::Duration),
    ) -> Option<SystemResult> {
        self.active_epoch.as_ref()?;
        let env = active::ActiveStepEnv {
            triggers,
            zoom,
            now,
            in_flight,
            moving_this_frame: triggers.moving_this_frame,
            caller_immediate,
            view_valid,
            fade_zero: caller_immediate,
            earlier_stage_spent,
        };
        active::handle_aborted_or_deferred_active_epoch(self, ctx, &env)
    }

    fn run_placement_step(
        &mut self,
        ctx: &mut MapContext,
        config: PlacementConfig,
        caller_immediate: bool,
    ) -> SystemResult {
        let now = ctx
            .world
            .resources
            .get::<crate::render::frame_input::FrameInput>()
            .map_or(std::time::Duration::ZERO, |input| input.timestamp);
        let in_flight = prepare_boot_and_clock(self, ctx, now, caller_immediate);
        let zoom = ctx.view_state.style_zoom().value();
        let (content_sig, terrain_sig) =
            active::resolve_signatures(self, &ctx.world, &ctx.style, zoom);
        let (triggers, view, view_valid, obstacles) =
            self.build_triggers(ctx, content_sig, terrain_sig);

        let earlier_stage_spent = ctx
            .world
            .resources
            .get::<crate::tcs::system::timings::FrameTimings>()
            .map_or(
                std::time::Duration::ZERO,
                crate::tcs::system::timings::FrameTimings::elapsed_this_frame,
            );
        if let Some(res) = self.try_step_active_epoch(
            ctx,
            &triggers,
            (zoom, now, in_flight),
            (caller_immediate, view_valid, earlier_stage_spent),
        ) {
            return res;
        }
        if self.try_short_circuit_boot_loading(ctx, now, in_flight, &triggers) {
            return Ok(());
        }

        let mut sort_pending =
            self.maybe_sort_bearing(ctx, zoom, triggers.moving_this_frame, caller_immediate);
        let controller_wants = self.controller.should_place(now, zoom, config, triggers);
        let boot_final_settle = self.initial_boot_loading
            && in_flight == 0
            && (triggers.new_content
                || self.controller.is_pending()
                || !self.controller.is_initialized());
        if !(controller_wants || boot_final_settle)
            || (triggers.moving_this_frame
                && !caller_immediate
                && !self.initial_boot_loading
                && earlier_stage_spent > std::time::Duration::from_micros(2_200)
                && self.controller.defer_for_frame_budget(triggers))
        {
            self.run_skipped_or_deferred(ctx, now, in_flight, sort_pending);
            return Ok(());
        }

        start_new_epoch(
            self,
            ctx,
            EpochStartParams {
                view,
                view_valid,
                obstacles,
                content_sig,
                terrain_sig,
                triggers,
                zoom,
                now,
                in_flight,
                caller_immediate,
                boot_final_settle,
                fade_duration: config.fade_duration,
                sort_pending: &mut sort_pending,
            },
        )
    }
}

impl System for CollisionSystem {
    fn name(&self) -> Cow<'static, str> {
        "sdf_collision_system".into()
    }

    fn run(&mut self, ctx: &mut MapContext) -> SystemResult {
        if ctx.renderer.settings.symbol_pools.is_empty() {
            return Ok(());
        }
        if !super::style_has_symbol_layers(&ctx.style)
            && super::symbol_pool_empty(&ctx.world)
            && self.uploaded.is_empty()
            && self.active_epoch.is_none()
            && !self.history.is_fading()
        {
            return Ok(());
        }
        let config = ctx
            .world
            .resources
            .get::<PlacementConfig>()
            .copied()
            .unwrap_or_default();
        let caller_immediate = config.fade_duration.is_zero();
        if crate::render::eye_covering::EyeInFrame::reuses_content(&ctx.world) {
            active::sync_and_prune(
                &mut ctx.world,
                &ctx.renderer,
                &self.history,
                &self.uploaded,
                ctx.view_state.camera().get_pitch().0,
            );
            return Ok(());
        }
        self.run_placement_step(ctx, config, caller_immediate)
    }
}

struct EpochStartParams<'a> {
    view: ViewSignature,
    view_valid: bool,
    obstacles: u64,
    content_sig: u64,
    terrain_sig: u64,
    triggers: PlacementTriggers,
    zoom: f64,
    now: std::time::Duration,
    in_flight: usize,
    caller_immediate: bool,
    boot_final_settle: bool,
    fade_duration: std::time::Duration,
    sort_pending: &'a mut bool,
}

fn prepare_boot_and_clock(
    sys: &mut CollisionSystem,
    ctx: &mut MapContext,
    now: std::time::Duration,
    caller_immediate: bool,
) -> usize {
    let total_requests = crate::io::tile_backpressure::total_tile_requests(&ctx.world);
    let in_flight = crate::io::tile_backpressure::tiles_in_flight(&ctx.world);
    if !caller_immediate && !sys.initial_boot_finished && total_requests > 0 {
        sys.initial_boot_loading = true;
        if !now.is_zero() {
            sys.initial_boot_started_at.get_or_insert(now);
        }
    }
    let effective_fade_duration = if sys.initial_boot_loading {
        std::time::Duration::ZERO
    } else {
        ctx.world
            .resources
            .get::<PlacementConfig>()
            .map_or(std::time::Duration::from_millis(160), |c| c.fade_duration)
    };
    sys.history.set_fade_duration(effective_fade_duration);
    sys.history.advance_clock(now);
    if sys.history.rebase_clock_if_idle() {
        if let Some(Eventually::Initialized(pool)) =
            ctx.world.resources.get::<Eventually<SymbolBufferPool>>()
        {
            upload_diff::rebase_uploaded_metadata(
                pool,
                &ctx.renderer.queue,
                &mut sys.uploaded,
                &mut sys.scratch,
            );
        }
    }
    sys.runs = sys.runs.wrapping_add(1);
    let stats = ctx.world.resources.get_or_init_mut::<CollisionStats>();
    stats.runs = stats.runs.wrapping_add(1);
    sys.seen.clear();
    if let Some(covering) = ctx.world.resources.get::<super::covering::SymbolCovering>() {
        sys.seen.extend(covering.tiles.iter().copied());
    }
    in_flight
}

fn start_new_epoch(
    sys: &mut CollisionSystem,
    ctx: &mut MapContext,
    params: EpochStartParams<'_>,
) -> SystemResult {
    sys.obstacles = params.obstacles;
    if params.view_valid {
        sys.placed_view = Some(params.view);
    }
    sys.last_content_signature = Some(params.content_sig);
    sys.last_terrain_signature = Some(params.terrain_sig);
    sys.last_external_view = Some(params.triggers.external_view);

    let use_slicing = !params.caller_immediate
        && !params.boot_final_settle
        && sys.controller.last_dt_non_zero()
        && !params.triggers.obstructed
        && (sys.initial_boot_loading
            || sys.controller.has_ever_placed()
            || sys.controller.is_initialized());
    let bearing = ctx.view_state.camera().get_bearing().0;
    let mut layers = visible_layers(&ctx.world, &ctx.style, params.zoom, &sys.seen);
    if !use_slicing && (params.triggers.moving_this_frame || *params.sort_pending) {
        sys.draw_sort.follow_bearing(
            &ctx.world,
            &ctx.renderer.queue,
            (bearing, params.zoom),
            &layers,
            None,
        );
        *params.sort_pending = false;
    }
    layers.sort_by_key(|(index, layer, _)| {
        (
            std::cmp::Reverse(*index),
            std::cmp::Reverse(u8::from(layer.coords.z)),
            layer.coords.x,
            layer.coords.y,
        )
    });
    let projection = projection_data_for_view(&ctx.style, &ctx.view_state).map_err(|error| {
        tracing::error!(%error, "symbol projection failed");
        SystemError::Setup
    })?;
    let effective_fade = if sys.initial_boot_loading {
        std::time::Duration::ZERO
    } else {
        params.fade_duration
    };
    let has_symbols = !layers.is_empty();
    sys.history.begin_with_duration(params.now, effective_fade);
    sys.uploaded.retain(|(coords, _), cached| {
        let keep = sys.seen.contains(coords);
        if !keep {
            sys.scratch.recycle(std::mem::take(&mut cached.quads));
        }
        keep
    });
    let grid = std::mem::take(&mut sys.grid_cache);
    let epoch = sliced::ActiveEpoch::begin(
        &ctx.world,
        &ctx.style,
        &ctx.view_state,
        projection,
        layers,
        grid,
    );
    sys.grid_cache = active::finish_new_epoch(
        sys,
        ctx,
        epoch,
        (params.zoom, bearing, params.in_flight, *params.sort_pending),
        has_symbols,
        use_slicing,
        params.triggers.moving_this_frame,
    );
    Ok(())
}
