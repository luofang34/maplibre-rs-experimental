//! Active epoch stepping and deferred-frame synchronization for [`super::CollisionSystem`].

use std::time::Duration;

use super::{
    controller::PlacementTriggers, signatures, sliced, sync, temporal, upload_diff, CollisionGrid,
};
use crate::{
    context::MapContext,
    style::Style,
    tcs::{system::SystemResult, world::World},
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
    renderer: &crate::render::Renderer,
    history: &temporal::PlacementHistory,
    uploaded: &rustc_hash::FxHashMap<
        (crate::coords::WorldTileCoords, crate::style::StyleLayerId),
        upload_diff::CachedBucketMetadata,
    >,
    pitch: f64,
) {
    sync::sync_symbol_depth_fade(world, renderer, history, pitch);
    sync::prune_inactive_symbol_draws(world, history, uploaded);
}

pub(super) fn handle_aborted_or_deferred_active_epoch(
    sys: &mut super::CollisionSystem,
    ctx: &mut MapContext,
    env: &ActiveStepEnv<'_>,
) -> Option<SystemResult> {
    let stale_camera = sys.active_epoch.as_ref().is_some_and(|epoch| {
        let zoom_delta = (env.zoom - epoch.zoom).abs();
        let d = (ctx.view_state.camera().get_bearing().0 - epoch.bearing)
            .rem_euclid(std::f64::consts::TAU);
        let bearing_deg = d.min(std::f64::consts::TAU - d).to_degrees();
        zoom_delta > 0.35 || bearing_deg > 15.0
    });
    if env.triggers.obstructed || env.fade_zero || !env.view_valid || stale_camera {
        if let Some(epoch) = sys.active_epoch.take() {
            sys.grid_cache = epoch.abort(&mut sys.scratch);
            sys.scratch.shrink_transient();
        }
        sys.placed_view = None;
        sys.last_content_signature = None;
        if stale_camera
            && env.moving_this_frame
            && !env.caller_immediate
            && !env.triggers.obstructed
        {
            sys.controller.advance_clock(env.now);
            sys.controller.note_aborted_motion(env.zoom);
            sync_and_prune(
                &mut ctx.world,
                &ctx.renderer,
                &sys.history,
                &sys.uploaded,
                ctx.view_state.camera().get_pitch().0,
            );
            crate::render::frame_signals::keep_animating(&mut ctx.world);
            return Some(Ok(()));
        }
        return None;
    }
    step_existing_epoch(sys, ctx, env)
}

fn step_existing_epoch(
    sys: &mut super::CollisionSystem,
    ctx: &mut MapContext,
    env: &ActiveStepEnv<'_>,
) -> Option<SystemResult> {
    sys.controller.advance_clock(env.now);
    sys.controller
        .note_background_dirty(env.zoom, *env.triggers);
    let mut epoch = sys.active_epoch.take()?;
    if env.triggers.new_content || env.triggers.environment_changed {
        epoch.saw_background_change = true;
    }
    if env.moving_this_frame
        && !env.fade_zero
        && env.earlier_stage_spent > Duration::from_micros(2_000)
    {
        sys.active_epoch = Some(epoch);
        sync_and_prune(
            &mut ctx.world,
            &ctx.renderer,
            &sys.history,
            &sys.uploaded,
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
            history: &mut sys.history,
            uploaded: &mut sys.uploaded,
            scratch: &mut sys.scratch,
            sliced: true,
        },
        env.moving_this_frame,
    );
    finish_active_step(sys, ctx, env, epoch, committed)
}

fn finish_active_step(
    sys: &mut super::CollisionSystem,
    ctx: &mut MapContext,
    env: &ActiveStepEnv<'_>,
    mut epoch: sliced::ActiveEpoch,
    committed: bool,
) -> Option<SystemResult> {
    let bearing = ctx.view_state.camera().get_bearing().0;
    let sort_pending = if committed {
        let followup = sys.history.repeats.yielded_to_previous() || epoch.saw_background_change;
        sys.controller
            .record_commit(epoch.zoom, epoch.has_symbols, followup);
        let sort_budget = (!env.caller_immediate && sys.controller.last_dt_non_zero())
            .then_some((Duration::from_micros(400), 3));
        let sort_pending = if !env.moving_this_frame && sys.draw_sort.might_need_sort(bearing) {
            let layers = signatures::visible_layers(&ctx.world, &ctx.style, env.zoom, &sys.seen);
            if sys
                .draw_sort
                .needs_bearing_sort(&ctx.world, (bearing, env.zoom), &layers)
            {
                sys.draw_sort.follow_bearing(
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
        if sys.initial_boot_loading
            && !followup
            && !sort_pending
            && can_finish_initial_boot(&ctx.world, env.in_flight)
        {
            sys.initial_boot_finished = true;
            sys.initial_boot_loading = false;
        }
        ctx.world.resources.insert(epoch.take_placed());
        ctx.world
            .resources
            .insert(super::super::query::PlacementBearing(epoch.bearing));
        sys.grid_cache = epoch.take_grid();
        sys.scratch.shrink_transient();
        sort_pending
    } else {
        sys.active_epoch = Some(epoch);
        false
    };
    sync_and_prune(
        &mut ctx.world,
        &ctx.renderer,
        &sys.history,
        &sys.uploaded,
        ctx.view_state.camera().get_pitch().0,
    );
    if sys.active_epoch.is_some()
        || sys.history.is_fading()
        || sys.controller.is_pending()
        || sort_pending
    {
        crate::render::frame_signals::keep_animating(&mut ctx.world);
    }
    Some(Ok(()))
}

pub(super) fn resolve_signatures(
    sys: &mut super::CollisionSystem,
    world: &World,
    style: &Style,
    zoom: f64,
) -> (u64, u64) {
    sys.sig_cache.sync_style(style);
    let key = sys.sig_cache.content_key(world, style, zoom, &sys.seen);
    let content_sig = if let Some(sig) = sys.sig_cache.cached_content_sig(key) {
        sig
    } else {
        let sig = signatures::content_signature_for_seen(world, style, zoom, &sys.seen);
        sys.sig_cache.store_content_sig(key, sig);
        sig
    };
    let terrain_sig = signatures::terrain_signature(world);
    (content_sig, terrain_sig)
}

pub(super) fn finish_new_epoch(
    sys: &mut super::CollisionSystem,
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
            history: &mut sys.history,
            uploaded: &mut sys.uploaded,
            scratch: &mut sys.scratch,
            sliced: use_slicing,
        },
        moving_this_frame,
    );
    let mut returned_grid = CollisionGrid::new(0.0, 0.0);
    if committed {
        let followup_needed = sys.history.repeats.yielded_to_previous();
        sys.controller
            .record_commit(zoom, has_symbols, followup_needed);
        if sys.initial_boot_loading
            && !followup_needed
            && !sort_pending
            && can_finish_initial_boot(&ctx.world, in_flight)
        {
            sys.initial_boot_finished = true;
            sys.initial_boot_loading = false;
        }
        ctx.world.resources.insert(epoch.take_placed());
        ctx.world
            .resources
            .insert(super::super::query::PlacementBearing(bearing));
        returned_grid = epoch.take_grid();
        sys.scratch.shrink_transient();
    } else {
        sys.active_epoch = Some(epoch);
    }
    sync_and_prune(
        &mut ctx.world,
        &ctx.renderer,
        &sys.history,
        &sys.uploaded,
        ctx.view_state.camera().get_pitch().0,
    );
    if sys.active_epoch.is_some()
        || sys.history.is_fading()
        || sys.controller.is_pending()
        || sort_pending
    {
        crate::render::frame_signals::keep_animating(&mut ctx.world);
    }
    returned_grid
}

pub(super) fn can_finish_initial_boot(world: &crate::tcs::world::World, in_flight: usize) -> bool {
    in_flight == 0
        && !crate::io::tile_backpressure::has_unrequested_visible_tiles(world)
        && !world
            .resources
            .get::<crate::sdf::upload_system::SdfUploadPending>()
            .is_some_and(|p| p.0)
}
