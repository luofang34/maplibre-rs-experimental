//! Places elevated text and icons on discrete placement epochs, interpolating fades on the GPU.
use std::borrow::Cow;

pub use controller::PlacementConfig;
use controller::{PlacementController, PlacementTriggers};
use rustc_hash::{FxHashMap, FxHashSet};
#[cfg(test)]
use signatures::obstructed_grid;
use signatures::{
    content_signature, has_symbol_depth_occluders, terrain_signature, visible_layers, VisibleLayer,
};

#[cfg(test)]
use super::placement::canonical_tile;
use crate::{
    context::MapContext,
    coords::WorldTileCoords,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::projection_data_for_view,
    },
    sdf::SymbolBufferPool,
    tcs::system::{System, SystemError, SystemResult},
};

mod controller;
mod draw_sort;
mod family;
mod layer_pass;
mod repeat_spacing;
mod rules;
mod signatures;
mod sliced;
mod temporal;
mod upload_diff;

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
}

impl System for CollisionSystem {
    fn name(&self) -> Cow<'static, str> {
        "sdf_collision_system".into()
    }

    fn run(
        &mut self,
        MapContext {
            world,
            style,
            view_state,
            renderer,
            ..
        }: &mut MapContext,
    ) -> SystemResult {
        if renderer.settings.symbol_pools.is_empty() {
            return Ok(());
        }
        if !super::style_has_symbol_layers(style)
            && super::symbol_pool_empty(world)
            && self.uploaded.is_empty()
            && self.active_epoch.is_none()
            && !self.history.is_fading()
        {
            return Ok(());
        }
        let config = world
            .resources
            .get::<PlacementConfig>()
            .copied()
            .unwrap_or_default();
        let caller_immediate = config.fade_duration.is_zero();
        if crate::render::eye_covering::EyeInFrame::reuses_content(world) {
            sync_symbol_depth_fade(
                world,
                &renderer.queue,
                &renderer.render_graph,
                &self.history,
                view_state.camera().get_pitch().0,
            );
            self.prune_inactive_symbol_draws(world);
            return Ok(());
        }

        let now = world
            .resources
            .get::<crate::render::frame_input::FrameInput>()
            .map_or(std::time::Duration::ZERO, |input| input.timestamp);
        let total_requests = crate::io::tile_backpressure::total_tile_requests(world);
        let in_flight = crate::io::tile_backpressure::tiles_in_flight(world);
        if !caller_immediate && !self.initial_boot_finished && total_requests > 0 {
            self.initial_boot_loading = true;
            if !now.is_zero() {
                self.initial_boot_started_at.get_or_insert(now);
            }
        }
        let effective_fade_duration = if self.initial_boot_loading {
            std::time::Duration::ZERO
        } else {
            config.fade_duration
        };

        self.history.set_fade_duration(effective_fade_duration);
        self.history.advance_clock(now);

        self.runs = self.runs.wrapping_add(1);
        let stats = world.resources.get_or_init_mut::<CollisionStats>();
        stats.runs = stats.runs.wrapping_add(1);

        self.seen.clear();
        if let Some(covering) = world.resources.get::<super::covering::SymbolCovering>() {
            self.seen.extend(covering.tiles.iter().copied());
        }
        let zoom = view_state.style_zoom().value();
        let mut layers = visible_layers(world, style, zoom, &self.seen);

        let content_sig = content_signature(world, &layers);
        let terrain_sig = terrain_signature(world);
        let external_view = view_state.has_external_view();
        let view = (view_state.view_projection().0, view_state.viewport_size());
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

        let obstacles = world
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

        let earlier_stage_spent = world
            .resources
            .get::<crate::tcs::system::timings::FrameTimings>()
            .map_or(
                std::time::Duration::ZERO,
                crate::tcs::system::timings::FrameTimings::elapsed_this_frame,
            );
        if self.active_epoch.is_some() {
            let stale_camera = self.active_epoch.as_ref().is_some_and(|epoch| {
                let zoom_delta = (zoom - epoch.zoom).abs();
                let d = (view_state.camera().get_bearing().0 - epoch.bearing)
                    .rem_euclid(std::f64::consts::TAU);
                let bearing_deg = d.min(std::f64::consts::TAU - d).to_degrees();
                zoom_delta > 0.35 || bearing_deg > 15.0
            });
            if triggers.obstructed || config.fade_duration.is_zero() || !view_valid || stale_camera
            {
                if let Some(epoch) = self.active_epoch.take() {
                    epoch.abort(&mut self.scratch);
                }
                self.placed_view = None;
                self.last_content_signature = None;
            } else {
                self.controller.advance_clock(now);
                self.controller.note_background_dirty(zoom, triggers);
                let Some(mut epoch) = self.active_epoch.take() else {
                    return Ok(());
                };
                if triggers.new_content || triggers.environment_changed {
                    epoch.saw_background_change = true;
                }
                // If earlier upload/drape stages already spent >2 ms while the camera is actively
                // moving, defer this symbol collision slice to the next frame to prevent combined
                // upload + placement spikes.
                if moving_this_frame
                    && !config.fade_duration.is_zero()
                    && earlier_stage_spent > std::time::Duration::from_micros(2_000)
                {
                    self.active_epoch = Some(epoch);
                    sync_symbol_depth_fade(
                        world,
                        &renderer.queue,
                        &renderer.render_graph,
                        &self.history,
                        view_state.camera().get_pitch().0,
                    );
                    self.prune_inactive_symbol_draws(world);
                    crate::render::frame_signals::keep_animating(world);
                    return Ok(());
                }
                if epoch.step(
                    world,
                    style,
                    &renderer.queue,
                    &mut self.history,
                    &mut self.uploaded,
                    &mut self.scratch,
                    true,
                    moving_this_frame,
                ) {
                    let followup =
                        self.history.repeats.yielded_to_previous() || epoch.saw_background_change;
                    self.controller
                        .record_commit(epoch.zoom, epoch.has_symbols, followup);
                    if !moving_this_frame
                        && self.draw_sort.needs_bearing_sort(
                            world,
                            (view_state.camera().get_bearing().0, zoom),
                            &layers,
                        )
                    {
                        self.draw_sort.follow_bearing(
                            world,
                            &renderer.queue,
                            (view_state.camera().get_bearing().0, zoom),
                            &layers,
                        );
                    }
                    world.resources.insert(epoch.take_placed());
                    world
                        .resources
                        .insert(super::query::PlacementBearing(epoch.bearing));
                } else {
                    self.active_epoch = Some(epoch);
                }
                sync_symbol_depth_fade(
                    world,
                    &renderer.queue,
                    &renderer.render_graph,
                    &self.history,
                    view_state.camera().get_pitch().0,
                );
                self.prune_inactive_symbol_draws(world);
                if self.active_epoch.is_some()
                    || self.history.is_fading()
                    || self.controller.is_pending()
                {
                    crate::render::frame_signals::keep_animating(world);
                }
                return Ok(());
            }
        }

        if self.initial_boot_loading
            && in_flight > 0
            && !moving_this_frame
            && !triggers.obstructed
            && self.initial_boot_started_at.is_some_and(|start| {
                now.saturating_sub(start) < std::time::Duration::from_millis(250)
            })
        {
            self.controller.advance_clock(now);
            sync_symbol_depth_fade(
                world,
                &renderer.queue,
                &renderer.render_graph,
                &self.history,
                view_state.camera().get_pitch().0,
            );
            self.prune_inactive_symbol_draws(world);
            return Ok(());
        }

        let bearing = view_state.camera().get_bearing().0;
        if !moving_this_frame
            && self
                .draw_sort
                .needs_bearing_sort(world, (bearing, zoom), &layers)
        {
            self.draw_sort
                .follow_bearing(world, &renderer.queue, (bearing, zoom), &layers);
        }

        let controller_wants = self.controller.should_place(now, zoom, config, triggers);
        let boot_final_settle = self.initial_boot_loading
            && in_flight == 0
            && (triggers.new_content
                || self.controller.is_pending()
                || !self.controller.is_initialized());
        if !(controller_wants || boot_final_settle)
            || (moving_this_frame
                && !caller_immediate
                && !self.initial_boot_loading
                && earlier_stage_spent > std::time::Duration::from_micros(2_200)
                && self.controller.defer_for_frame_budget(triggers))
        {
            sync_symbol_depth_fade(
                world,
                &renderer.queue,
                &renderer.render_graph,
                &self.history,
                view_state.camera().get_pitch().0,
            );
            self.prune_inactive_symbol_draws(world);
            if self.history.is_fading() || self.controller.is_pending() {
                crate::render::frame_signals::keep_animating(world);
            } else if self.initial_boot_loading && in_flight == 0 {
                self.initial_boot_finished = true;
                self.initial_boot_loading = false;
            }
            return Ok(());
        }

        self.obstacles = obstacles;
        if view_valid {
            self.placed_view = Some(view);
        }
        self.last_content_signature = Some(content_sig);
        self.last_terrain_signature = Some(terrain_sig);
        self.last_external_view = Some(external_view);

        let use_slicing = self.controller.is_initialized()
            && self.controller.last_dt_non_zero()
            && !effective_fade_duration.is_zero()
            && !triggers.obstructed;
        if !moving_this_frame || !use_slicing {
            self.draw_sort
                .follow_bearing(world, &renderer.queue, (bearing, zoom), &layers);
        }
        layers.sort_by_key(|(index, layer, _)| {
            (
                std::cmp::Reverse(*index),
                std::cmp::Reverse(u8::from(layer.coords.z)),
                layer.coords.x,
                layer.coords.y,
            )
        });
        let projection = projection_data_for_view(style, view_state).map_err(|error| {
            tracing::error!(%error, "symbol projection failed");
            SystemError::Setup
        })?;
        let has_symbols = !layers.is_empty();
        self.history
            .begin_with_duration(now, effective_fade_duration);
        self.uploaded.retain(|(coords, _), cached| {
            let keep = self.seen.contains(coords);
            if !keep {
                self.scratch.recycle(std::mem::take(&mut cached.vertices));
            }
            keep
        });
        let mut epoch = sliced::ActiveEpoch::begin(world, style, view_state, projection, layers);
        if epoch.step(
            world,
            style,
            &renderer.queue,
            &mut self.history,
            &mut self.uploaded,
            &mut self.scratch,
            use_slicing,
            moving_this_frame,
        ) {
            let followup_needed = self.history.repeats.yielded_to_previous();
            self.controller
                .record_commit(zoom, has_symbols, followup_needed);
            if self.initial_boot_loading && in_flight == 0 && !followup_needed {
                self.initial_boot_finished = true;
                self.initial_boot_loading = false;
            }
            world.resources.insert(epoch.take_placed());
            world
                .resources
                .insert(super::query::PlacementBearing(bearing));
        } else {
            self.active_epoch = Some(epoch);
        }
        sync_symbol_depth_fade(
            world,
            &renderer.queue,
            &renderer.render_graph,
            &self.history,
            view_state.camera().get_pitch().0,
        );
        self.prune_inactive_symbol_draws(world);
        if self.active_epoch.is_some() || self.history.is_fading() || self.controller.is_pending() {
            crate::render::frame_signals::keep_animating(world);
        }
        Ok(())
    }
}

fn sync_symbol_depth_fade(
    world: &mut crate::tcs::world::World,
    queue: &crate::render::upload_queue::UploadQueue,
    render_graph: &crate::render::graph::RenderGraph,
    history: &temporal::PlacementHistory,
    pitch: f64,
) {
    let time_ms = history.effective_shader_time_ms();
    let duration_ms = history.fade_duration_ms();
    let skip_depth_occlusion = !has_symbol_depth_occluders(world, render_graph, pitch);
    if let Some(Initialized(depth)) = world
        .resources
        .get_mut::<Eventually<super::depth::SymbolDepth>>()
    {
        depth.write_fade_time(queue, time_ms, duration_ms, skip_depth_occlusion);
    }
}

impl CollisionSystem {
    fn prune_inactive_symbol_draws(&self, world: &mut crate::tcs::world::World) {
        use crate::{
            render::render_phase::{Draw, DrawState, RenderPhase, TranslucentItem},
            sdf::render_commands::DrawSymbols,
        };
        let now_ms = self.history.effective_shader_time_ms();
        let fade_duration_ms = self.history.fade_duration_ms();
        let Some((phase, Initialized(pool))) = world.resources.query_mut::<(
            &mut RenderPhase<TranslucentItem>,
            &Eventually<SymbolBufferPool>,
        )>() else {
            return;
        };
        let symbol_draw: &dyn Draw<TranslucentItem> =
            &DrawState::<TranslucentItem, DrawSymbols>::INSTANCE;
        phase.retain(|item| {
            if !std::ptr::addr_eq(item.draw_function, symbol_draw) {
                return true;
            }
            let Some(entry) =
                pool.index()
                    .resolve_entry(item.tile.coords, item.style_layer, item.entry_slot)
            else {
                return false;
            };
            self.uploaded
                .get(&(item.tile.coords, item.style_layer))
                .is_some_and(|cached| {
                    cached.allocation_id == entry.allocation_id()
                        && cached.is_active_at(now_ms, fade_duration_ms)
                })
        });
    }
}
