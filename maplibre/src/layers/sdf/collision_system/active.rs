//! Active epoch stepping and deferred-frame synchronization for [`super::CollisionSystem`].

use std::time::Duration;

use rustc_hash::FxHashMap;

use super::{
    controller::PlacementTriggers, signatures, sliced, temporal, upload_diff, CollisionGrid,
};
use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        render_phase::{RenderPhase, TranslucentItem},
        Renderer,
    },
    schedule::system::SystemResult,
    sdf::{depth::SymbolDepth, SymbolBufferPool},
    style::{Style, StyleLayerId},
    tcs::world::World,
};

pub(super) struct ActiveStepEnv<'a> {
    pub(super) triggers: &'a PlacementTriggers,
    pub(super) zoom: f64,
    pub(super) now: Duration,
    pub(super) in_flight: usize,
    pub(super) moving_this_frame: bool,
    pub(super) caller_immediate: bool,
    pub(super) view_valid: bool,
    pub(super) fade_zero: bool,
    pub(super) earlier_stage_spent: Duration,
}

pub(super) fn sync_and_prune(
    world: &mut World,
    renderer: &Renderer,
    history: &temporal::PlacementHistory,
    uploaded: &FxHashMap<(WorldTileCoords, StyleLayerId), upload_diff::CachedBucketMetadata>,
    pitch: f64,
) {
    sync_symbol_depth_fade(world, renderer, history, pitch);
    prune_inactive_symbol_draws(world, history, uploaded);
}

fn sync_symbol_depth_fade(
    world: &mut World,
    renderer: &Renderer,
    history: &temporal::PlacementHistory,
    pitch: f64,
) {
    let time_ms = history.effective_shader_time_ms();
    let duration_ms = history.fade_duration_ms();
    let skip_depth_occlusion =
        !signatures::has_symbol_depth_occluders(world, &renderer.render_graph, pitch);
    if let Some(Initialized(depth)) = world.resources.get_mut::<Eventually<SymbolDepth>>() {
        depth.write_fade_time(
            &renderer.device,
            &renderer.queue,
            time_ms,
            duration_ms,
            skip_depth_occlusion,
        );
    }
}

fn prune_inactive_symbol_draws(
    world: &mut World,
    history: &temporal::PlacementHistory,
    uploaded: &FxHashMap<(WorldTileCoords, StyleLayerId), upload_diff::CachedBucketMetadata>,
) {
    let has_symbol_items = world
        .resources
        .get::<RenderPhase<TranslucentItem>>()
        .is_some_and(|phase| {
            phase
                .into_iter()
                .any(|item| item.style_layer != StyleLayerId::ATMOSPHERE && item.index != u32::MAX)
        });
    if !has_symbol_items {
        return;
    }
    let now_ms = history.effective_shader_time_ms();
    let fade_duration_ms = history.fade_duration_ms();
    let Some((phase, Initialized(pool))) = world.resources.query_mut::<(
        &mut RenderPhase<TranslucentItem>,
        &Eventually<SymbolBufferPool>,
    )>() else {
        return;
    };
    phase.retain(|item| {
        if item.style_layer == StyleLayerId::ATMOSPHERE || item.index == u32::MAX {
            return true;
        }
        let Some(entry) =
            pool.as_ref()
                .resolve_entry(item.tile.coords, item.style_layer, item.entry_slot)
        else {
            return false;
        };
        uploaded
            .get(&(item.tile.coords, item.style_layer))
            .is_some_and(|cached| {
                cached.allocation_id == entry.allocation_id()
                    && cached.is_active_at(now_ms, fade_duration_ms)
            })
    });
}

impl super::CollisionSystem {
    pub(super) fn handle_aborted_or_deferred_active_epoch(
        &mut self,
        ctx: &mut MapContext,
        env: &ActiveStepEnv<'_>,
    ) -> Option<SystemResult> {
        let stale_camera = self.active_epoch.as_ref().is_some_and(|epoch| {
            let zoom_delta = (env.zoom - epoch.zoom).abs();
            let d = (ctx.view_state.camera().get_bearing().0 - epoch.bearing)
                .rem_euclid(std::f64::consts::TAU);
            let bearing_deg = d.min(std::f64::consts::TAU - d).to_degrees();
            zoom_delta > 0.35 || bearing_deg > 15.0
        });
        if env.triggers.obstructed || env.fade_zero || !env.view_valid || stale_camera {
            if let Some(epoch) = self.active_epoch.take() {
                self.grid_cache = epoch.abort(&mut self.scratch);
                self.scratch.shrink_transient();
            }
            self.placed_view = None;
            self.last_content_signature = None;
            if stale_camera
                && env.moving_this_frame
                && !env.caller_immediate
                && !env.triggers.obstructed
            {
                self.controller.advance_clock(env.now);
                self.controller.note_aborted_motion(env.zoom);
                sync_and_prune(
                    &mut ctx.world,
                    &ctx.renderer,
                    &self.history,
                    &self.uploaded,
                    ctx.view_state.camera().get_pitch().0,
                );
                crate::render::frame_signals::keep_animating(&mut ctx.world);
                return Some(Ok(()));
            }
            return None;
        }
        self.step_existing_epoch(ctx, env)
    }

    fn step_existing_epoch(
        &mut self,
        ctx: &mut MapContext,
        env: &ActiveStepEnv<'_>,
    ) -> Option<SystemResult> {
        self.controller.advance_clock(env.now);
        self.controller
            .note_background_dirty(env.zoom, *env.triggers);
        let mut epoch = self.active_epoch.take()?;
        epoch.note_triggers(env.triggers);
        if env.moving_this_frame
            && !env.fade_zero
            && env.earlier_stage_spent > Duration::from_micros(2_000)
        {
            self.active_epoch = Some(epoch);
            sync_and_prune(
                &mut ctx.world,
                &ctx.renderer,
                &self.history,
                &self.uploaded,
                ctx.view_state.camera().get_pitch().0,
            );
            crate::render::frame_signals::keep_animating(&mut ctx.world);
            return Some(Ok(()));
        }
        let committed = epoch.step(
            sliced::EpochStepContext {
                world: &ctx.world,
                style: &ctx.style,
                queue: &ctx.renderer.queue,
                history: &mut self.history,
                uploaded: &mut self.uploaded,
                scratch: &mut self.scratch,
                sliced: true,
            },
            env.moving_this_frame,
        );
        self.finish_active_step(ctx, env, epoch, committed)
    }

    fn finish_active_step(
        &mut self,
        ctx: &mut MapContext,
        env: &ActiveStepEnv<'_>,
        epoch: sliced::ActiveEpoch,
        committed: bool,
    ) -> Option<SystemResult> {
        let bearing = ctx.view_state.camera().get_bearing().0;
        let sort_pending = if committed {
            let committed = epoch.into_committed();
            let followup =
                self.history.yielded_to_previous_repeat() || committed.saw_background_change;
            self.controller
                .record_commit(committed.zoom, committed.has_symbols, followup);
            let sort_budget = (!env.caller_immediate && self.controller.last_dt_non_zero())
                .then_some((Duration::from_micros(400), 3));
            let sort_pending = if !env.moving_this_frame && self.draw_sort.might_need_sort(bearing)
            {
                let layers =
                    signatures::visible_layers(&ctx.world, &ctx.style, env.zoom, &self.seen);
                if self
                    .draw_sort
                    .needs_bearing_sort(&ctx.world, (bearing, env.zoom), &layers)
                {
                    self.draw_sort.follow_bearing(
                        &ctx.world,
                        &ctx.renderer.queue,
                        (bearing, env.zoom),
                        &layers,
                        sort_budget,
                    )
                } else {
                    false
                }
            } else {
                false
            };
            if self.initial_boot_loading
                && !followup
                && !sort_pending
                && can_finish_initial_boot(&ctx.world, env.in_flight)
            {
                self.initial_boot_finished = true;
                self.initial_boot_loading = false;
            }
            ctx.world.resources.insert(committed.placed);
            ctx.world
                .resources
                .insert(super::super::query::PlacementBearing(committed.bearing));
            self.grid_cache = committed.grid;
            self.scratch.shrink_transient();
            sort_pending
        } else {
            self.active_epoch = Some(epoch);
            false
        };
        sync_and_prune(
            &mut ctx.world,
            &ctx.renderer,
            &self.history,
            &self.uploaded,
            ctx.view_state.camera().get_pitch().0,
        );
        if self.active_epoch.is_some()
            || self.history.is_fading()
            || self.controller.is_pending()
            || sort_pending
        {
            crate::render::frame_signals::keep_animating(&mut ctx.world);
        }
        Some(Ok(()))
    }

    pub(super) fn resolve_signatures(
        &mut self,
        world: &World,
        style: &Style,
        zoom: f64,
    ) -> (u64, u64) {
        let content_sig = self
            .sig_cache
            .resolve_content_sig(world, style, zoom, &self.seen);
        let terrain_sig = signatures::terrain_signature(world);
        (content_sig, terrain_sig)
    }

    pub(super) fn finish_new_epoch(
        &mut self,
        ctx: &mut MapContext,
        mut epoch: sliced::ActiveEpoch,
        (zoom, bearing, in_flight, sort_pending): (f64, f64, usize, bool),
        has_symbols: bool,
        use_slicing: bool,
        moving_this_frame: bool,
    ) -> CollisionGrid {
        let committed = epoch.step(
            sliced::EpochStepContext {
                world: &ctx.world,
                style: &ctx.style,
                queue: &ctx.renderer.queue,
                history: &mut self.history,
                uploaded: &mut self.uploaded,
                scratch: &mut self.scratch,
                sliced: use_slicing,
            },
            moving_this_frame,
        );
        let mut returned_grid = CollisionGrid::new(0.0, 0.0);
        if committed {
            let followup_needed = self.history.yielded_to_previous_repeat();
            self.controller
                .record_commit(zoom, has_symbols, followup_needed);
            if self.initial_boot_loading
                && !followup_needed
                && !sort_pending
                && can_finish_initial_boot(&ctx.world, in_flight)
            {
                self.initial_boot_finished = true;
                self.initial_boot_loading = false;
            }
            let committed = epoch.into_committed();
            ctx.world.resources.insert(committed.placed);
            ctx.world
                .resources
                .insert(super::super::query::PlacementBearing(bearing));
            returned_grid = committed.grid;
            self.scratch.shrink_transient();
        } else {
            self.active_epoch = Some(epoch);
        }
        sync_and_prune(
            &mut ctx.world,
            &ctx.renderer,
            &self.history,
            &self.uploaded,
            ctx.view_state.camera().get_pitch().0,
        );
        if self.active_epoch.is_some()
            || self.history.is_fading()
            || self.controller.is_pending()
            || sort_pending
        {
            crate::render::frame_signals::keep_animating(&mut ctx.world);
        }
        returned_grid
    }
}

pub(super) fn can_finish_initial_boot(world: &crate::tcs::world::World, in_flight: usize) -> bool {
    in_flight == 0
        && !crate::io::tile_backpressure::has_unrequested_visible_tiles(world)
        && crate::io::tile_retry::next_retry_delay(world).is_none()
        && !crate::sdf::provided::awaiting(world)
        && !world
            .resources
            .get::<crate::sdf::upload_system::SdfUploadPending>()
            .is_some_and(|p| p.0)
}
