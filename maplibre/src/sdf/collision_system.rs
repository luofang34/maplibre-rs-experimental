//! Places elevated text and icons on discrete placement epochs, interpolating fades on the GPU.
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
};

pub use controller::PlacementConfig;
use controller::{PlacementController, PlacementTriggers};
use layer_pass::place_layer;
use rustc_hash::{FxHashMap, FxHashSet};
use signatures::{
    content_signature, obstructed_grid, terrain_signature, visible_layers, VisibleLayer,
};

#[cfg(test)]
use super::placement::canonical_tile;
use super::query::PlacedSymbols;
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
    /// Revision of the host's label obstacles the last placement kept clear.
    obstacles: u64,
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
        let config = world
            .resources
            .get::<PlacementConfig>()
            .copied()
            .unwrap_or_default();
        if crate::render::eye_covering::EyeInFrame::reuses_content(world) {
            sync_symbol_depth_fade(world, &renderer.queue, &self.history);
            self.prune_inactive_symbol_draws(world);
            return Ok(());
        }

        let now = world
            .resources
            .get::<crate::render::frame_input::FrameInput>()
            .map_or(std::time::Duration::ZERO, |input| input.timestamp);
        self.history.set_fade_duration(config.fade_duration);
        self.history.advance_clock(now);

        self.runs = self.runs.wrapping_add(1);
        let stats = world.resources.get_or_init_mut::<CollisionStats>();
        stats.runs = stats.runs.wrapping_add(1);

        let seen: FxHashSet<_> = world
            .resources
            .get::<super::covering::SymbolCovering>()
            .map(|covering| covering.tiles.iter().copied().collect())
            .unwrap_or_default();
        let zoom = view_state.style_zoom().value();
        let mut layers = visible_layers(world, style, zoom, &seen);

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
        if !self.controller.should_place(now, zoom, config, triggers)
            || (moving_this_frame
                && !config.fade_duration.is_zero()
                && earlier_stage_spent > std::time::Duration::from_millis(4)
                && self.controller.defer_for_frame_budget(triggers))
        {
            sync_symbol_depth_fade(world, &renderer.queue, &self.history);
            self.prune_inactive_symbol_draws(world);
            if self.history.is_fading() || self.controller.is_pending() {
                crate::render::frame_signals::keep_animating(world);
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

        self.draw_sort.follow_bearing(
            world,
            &renderer.queue,
            (view_state.camera().get_bearing().0, zoom),
            &layers,
        );
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
        self.history.begin_with_duration(now, config.fade_duration);
        self.uploaded.retain(|(coords, _), cached| {
            let keep = seen.contains(coords);
            if !keep {
                self.scratch.recycle(std::mem::take(&mut cached.vertices));
            }
            keep
        });
        let placed = self.place_layers(
            world,
            style,
            view_state,
            &projection,
            &renderer.queue,
            layers,
        );
        let followup_needed = self.history.repeats.yielded_to_previous();
        self.controller
            .record_commit(zoom, has_symbols, followup_needed);

        world.resources.insert(placed);
        world.resources.insert(super::query::PlacementBearing(
            view_state.camera().get_bearing().0,
        ));
        sync_symbol_depth_fade(world, &renderer.queue, &self.history);
        self.prune_inactive_symbol_draws(world);
        if self.history.is_fading() || self.controller.is_pending() {
            crate::render::frame_signals::keep_animating(world);
        }
        Ok(())
    }
}

fn sync_symbol_depth_fade(
    world: &mut crate::tcs::world::World,
    queue: &crate::render::upload_queue::UploadQueue,
    history: &temporal::PlacementHistory,
) {
    let time_ms = history.effective_shader_time_ms();
    let duration_ms = history.fade_duration_ms();
    if let Some(Initialized(depth)) = world
        .resources
        .get_mut::<Eventually<super::depth::SymbolDepth>>()
    {
        depth.write_fade_time(queue, time_ms, duration_ms);
    }
}

impl CollisionSystem {
    fn place_layers(
        &mut self,
        world: &crate::tcs::world::World,
        style: &crate::style::Style,
        view_state: &crate::render::view_state::ViewState,
        projection: &crate::render::projection::ShaderProjectionData,
        queue: &crate::render::upload_queue::UploadQueue,
        layers: Vec<VisibleLayer<'_>>,
    ) -> PlacedSymbols {
        let mut boxes = obstructed_grid(
            view_state.width(),
            view_state.height(),
            world.resources.get::<super::obstacles::LabelObstacles>(),
        );
        let mut placed = PlacedSymbols::default();
        let mut groups: Vec<(
            u32,
            &crate::style::layer::SymbolPaint,
            Vec<&crate::sdf::SymbolLayerData>,
        )> = Vec::new();
        for (index, layer, paint) in layers {
            match groups.last_mut() {
                Some((last, _, members))
                    if *last == index && members[0].style_layer_id == layer.style_layer_id =>
                {
                    members.push(layer);
                }
                _ => groups.push((index, paint, vec![layer])),
            }
        }
        // A layer GL JS would group with a lower one is placed by it, after the pass.
        let leaders = family::leaders(style);
        let present: HashSet<&str> = groups
            .iter()
            .map(|(_, _, members)| members[0].style_layer_id.as_str())
            .collect();
        let mut outcomes = HashMap::new();
        let mut mirrored = Vec::new();
        let mut placed_layers = Vec::new();
        for (_, paint, members) in groups {
            let id = members[0].style_layer_id.clone();
            if let Some(leader) = leaders
                .get(&id)
                .filter(|leader| **leader != id && present.contains(leader.as_str()))
            {
                mirrored.push((leader.clone(), paint, members));
                continue;
            }
            let limits = style
                .layers
                .iter()
                .find(|style| style.id == id)
                .map(|layer| [layer.minzoom.unwrap_or(0.0), layer.maxzoom.unwrap_or(24.0)])
                .unwrap_or([0.0, 24.0]);
            let (metadata, outcome) = place_layer(
                world,
                view_state,
                projection,
                &members,
                paint,
                limits,
                (
                    &mut boxes,
                    &mut placed,
                    &mut self.history,
                    &mut self.scratch,
                ),
            );
            if leaders.get(&id) == Some(&id) {
                outcomes.insert(id, (members.clone(), outcome));
            }
            placed_layers.push((members, metadata));
        }
        for (leader, paint, members) in mirrored {
            let Some((leader_members, outcome)) = outcomes.get(&leader) else {
                continue;
            };
            let metadata = family::mirror(
                (leader_members, outcome),
                &members,
                (paint, view_state.style_zoom().value()),
                &mut placed,
            );
            placed_layers.push((members, metadata));
        }
        if let Some(Initialized(pool)) = world.resources.get::<Eventually<SymbolBufferPool>>() {
            for (members, metadata) in placed_layers {
                for (layer, metadata) in members.into_iter().zip(metadata) {
                    let layer_id = crate::style::StyleLayerId::intern(&layer.style_layer_id);
                    let Some(entry) = pool.index().get_layers(layer.coords).and_then(|entries| {
                        entries
                            .iter()
                            .find(|entry| entry.style_layer_id == layer_id)
                    }) else {
                        self.scratch.recycle(metadata);
                        continue;
                    };
                    let key = (layer.coords, layer_id);
                    let prev = self.uploaded.remove(&key);
                    let next_cached = upload_diff::diff_and_upload_bucket_with_scratch(
                        pool,
                        queue,
                        entry,
                        prev.as_ref(),
                        metadata,
                        &mut self.scratch,
                    );
                    if let Some(prev) = prev {
                        self.scratch.recycle(prev.vertices);
                    }
                    self.uploaded.insert(key, next_cached);
                }
            }
        }
        placed
    }

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
