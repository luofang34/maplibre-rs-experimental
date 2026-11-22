//! Time-sliced symbol collision epochs (`<= 1.8 ms` per-frame render-worker CPU budget).
//!
//! During continuous interactive camera movement, evaluating thousands of CJK/multi-script
//! glyphs across 10–20 style layers in a single frame causes 15–40 ms tail spikes. `ActiveEpoch`
//! evaluates symbol layers and 32-feature batches incrementally across consecutive frames against
//! a consistent `(ViewState, ShaderProjectionData)` snapshot, then commits `PlacedSymbols` and
//! dirty GPU `SDFShaderFeatureMetadata` runs atomically when the final group completes.

use std::{collections::VecDeque, time::Duration};

use rustc_hash::{FxHashMap, FxHashMap as HashMap, FxHashSet as HashSet};

use super::{
    family,
    layer_pass::slice::{LayerSliceProgress, SlicePlacementState, SliceStepContext},
    signatures::{populate_obstructed_grid, VisibleLayer},
    temporal, upload_diff,
};
use crate::{
    coords::WorldTileCoords,
    render::{
        eventually::{Eventually, Eventually::Initialized},
        projection::ShaderProjectionData,
        shaders::SDFShaderFeatureMetadata,
        upload_queue::UploadQueue,
        view_state::ViewState,
    },
    sdf::{
        collision_grid::CollisionGrid, query::PlacedSymbols, SymbolBufferPool, SymbolLayerData,
        SymbolLayersDataComponent,
    },
    style::{
        layer::{LayerPaint, SymbolPaint},
        Style, StyleLayerId,
    },
    tcs::world::World,
};

#[cfg(target_arch = "wasm32")]
const SLICE_BUDGET_SETTLED: Duration = Duration::from_micros(1000);
#[cfg(not(target_arch = "wasm32"))]
const SLICE_BUDGET_SETTLED: Duration = Duration::from_micros(1800);
#[cfg(target_arch = "wasm32")]
const SLICE_BUDGET_MOVING: Duration = Duration::from_micros(500);
#[cfg(not(target_arch = "wasm32"))]
const SLICE_BUDGET_MOVING: Duration = Duration::from_micros(550);

struct EpochGroup {
    style_layer_id: String,
    zoom_limits: [f64; 2],
    is_leader: bool,
    leader_of: Option<String>,
    coords: Vec<WorldTileCoords>,
}

/// Multi-frame incremental symbol collision epoch.
pub(super) struct ActiveEpoch {
    view_state: ViewState,
    projection: ShaderProjectionData,
    pub(super) zoom: f64,
    pub(super) bearing: f64,
    pub(super) has_symbols: bool,
    pub(super) saw_background_change: bool,
    boxes: CollisionGrid,
    placed: PlacedSymbols,
    groups: Vec<EpochGroup>,
    group_idx: usize,
    active_group: Option<LayerSliceProgress>,
    outcomes: HashMap<String, (Vec<WorldTileCoords>, family::Outcomes)>,
    mirrored: VecDeque<(String, String, Vec<WorldTileCoords>)>,
    pending_uploads: VecDeque<(StyleLayerId, WorldTileCoords, Vec<SDFShaderFeatureMetadata>)>,
}

/// Shared frame resources and caches used while stepping a sliced collision epoch.
pub(super) struct EpochStepContext<'a> {
    pub(super) world: &'a World,
    pub(super) style: &'a Style,
    pub(super) queue: &'a UploadQueue,
    pub(super) history: &'a mut temporal::PlacementHistory,
    pub(super) uploaded:
        &'a mut FxHashMap<(WorldTileCoords, StyleLayerId), upload_diff::CachedBucketMetadata>,
    pub(super) scratch: &'a mut upload_diff::MetadataScratchPool,
    pub(super) sliced: bool,
}

impl ActiveEpoch {
    pub(super) fn begin(
        world: &World,
        style: &Style,
        view_state: &ViewState,
        projection: ShaderProjectionData,
        layers: Vec<VisibleLayer<'_>>,
        mut boxes: CollisionGrid,
    ) -> Self {
        populate_obstructed_grid(
            &mut boxes,
            view_state.width(),
            view_state.height(),
            world
                .resources
                .get::<crate::sdf::obstacles::LabelObstacles>(),
        );
        let has_symbols = !layers.is_empty();
        let mut raw_groups: Vec<(u32, String, Vec<WorldTileCoords>)> = Vec::new();
        for (index, layer, _) in layers {
            match raw_groups.last_mut() {
                Some((last, id, coords)) if *last == index && *id == layer.style_layer_id => {
                    coords.push(layer.coords);
                }
                _ => raw_groups.push((index, layer.style_layer_id.clone(), vec![layer.coords])),
            }
        }
        let leaders = style.cached_symbol_leaders(family::leaders);
        let layer_by_id = style.cached_layer_index_by_id();
        let present: HashSet<String> = raw_groups.iter().map(|(_, id, _)| id.clone()).collect();
        let groups = raw_groups
            .into_iter()
            .map(|(_, id, coords)| {
                let leader_of = leaders
                    .get(&id)
                    .filter(|leader| **leader != id && present.contains(*leader))
                    .cloned();
                let is_leader = leaders.get(&id) == Some(&id);
                let zoom_limits = layer_by_id
                    .get(&id)
                    .and_then(|&idx| style.layers.get(idx))
                    .map(|l| {
                        [
                            l.minzoom.filter(|&m| m > 0.0).unwrap_or(f64::NEG_INFINITY),
                            l.maxzoom.unwrap_or(24.0),
                        ]
                    })
                    .unwrap_or([f64::NEG_INFINITY, 24.0]);
                EpochGroup {
                    style_layer_id: id,
                    zoom_limits,
                    is_leader,
                    leader_of,
                    coords,
                }
            })
            .collect();

        Self {
            view_state: view_state.clone(),
            projection,
            zoom: view_state.style_zoom().value(),
            bearing: view_state.camera().get_bearing().0,
            has_symbols,
            saw_background_change: false,
            boxes,
            placed: PlacedSymbols::default(),
            groups,
            group_idx: 0,
            active_group: None,
            outcomes: HashMap::default(),
            mirrored: VecDeque::new(),
            pending_uploads: VecDeque::new(),
        }
    }

    /// Steps this epoch within `SLICE_BUDGET_MOVING` (`500 µs` on `wasm32` while panning) or
    /// `SLICE_BUDGET_SETTLED` (`1.0 ms` on `wasm32` / `1.8 ms` native) when `sliced` is `true`
    /// (or runs all groups in one pass when `sliced` is `false`), returning `true` once all
    /// groups have been placed and uploaded atomically.
    pub(super) fn step(&mut self, ctx: EpochStepContext<'_>, moving_this_frame: bool) -> bool {
        let budget = if moving_this_frame {
            SLICE_BUDGET_MOVING
        } else {
            SLICE_BUDGET_SETTLED
        };
        self.step_with_budget(ctx, budget)
    }

    pub(super) fn step_with_budget(
        &mut self,
        mut ctx: EpochStepContext<'_>,
        budget: Duration,
    ) -> bool {
        let started = instant::Instant::now();
        let mut did_work = false;

        while let Some(group) = self.groups.get_mut(self.group_idx) {
            if ctx.sliced && self.active_group.is_none() && did_work && started.elapsed() >= budget
            {
                return false;
            }
            if let Some(leader) = &group.leader_of {
                self.mirrored.push_back((
                    leader.clone(),
                    group.style_layer_id.clone(),
                    std::mem::take(&mut group.coords),
                ));
                self.group_idx += 1;
                continue;
            }
            let Some(paint) = resolve_paint(ctx.style, &group.style_layer_id) else {
                self.group_idx += 1;
                continue;
            };
            did_work |= self.ensure_active_group(ctx.world, paint, ctx.history);
            let Some(progress) = self.step_active_group(&mut ctx, paint, started, budget) else {
                return false;
            };
            if let Some(progress) = progress {
                self.commit_finished_group(progress);
            }
            self.group_idx += 1;
        }

        self.finish_mirrored_and_upload(ctx, started, budget, did_work)
    }

    fn ensure_active_group(
        &mut self,
        world: &World,
        paint: &SymbolPaint,
        history: &temporal::PlacementHistory,
    ) -> bool {
        if self.active_group.is_some() {
            return false;
        }
        let Some(group) = self.groups.get_mut(self.group_idx) else {
            return false;
        };
        let mut live_coords = Vec::with_capacity(group.coords.len());
        let mut live_refs = Vec::with_capacity(group.coords.len());
        for &coords in &group.coords {
            if let Some(layer) = resolve_layer(world, coords, &group.style_layer_id) {
                live_coords.push(coords);
                live_refs.push(layer);
            }
        }
        group.coords = live_coords;
        self.active_group = Some(LayerSliceProgress::start(
            world,
            &self.view_state,
            &self.projection,
            &live_refs,
            paint,
            history,
        ));
        true
    }

    fn step_active_group(
        &mut self,
        ctx: &mut EpochStepContext<'_>,
        paint: &SymbolPaint,
        started: instant::Instant,
        budget: Duration,
    ) -> Option<Option<LayerSliceProgress>> {
        let Some(group) = self.groups.get(self.group_idx) else {
            return Some(None);
        };
        let resolved: Vec<Option<&SymbolLayerData>> = group
            .coords
            .iter()
            .map(|&coords| resolve_layer(ctx.world, coords, &group.style_layer_id))
            .collect();
        let Some(mut progress) = self.active_group.take() else {
            return Some(None);
        };
        let sliced = ctx.sliced;
        let finished = progress.step(
            SliceStepContext {
                world: ctx.world,
                view_state: &self.view_state,
                projection: &self.projection,
                layers: &resolved,
                paint,
                zoom_limits: group.zoom_limits,
            },
            SlicePlacementState {
                boxes: &mut self.boxes,
                placed: &mut self.placed,
                history: ctx.history,
                scratch: ctx.scratch,
            },
            || sliced && started.elapsed() >= budget,
        );
        if !finished {
            self.active_group = Some(progress);
            return None;
        }
        Some(Some(progress))
    }

    fn commit_finished_group(&mut self, progress: LayerSliceProgress) {
        let Some(group) = self.groups.get_mut(self.group_idx) else {
            return;
        };
        let coords = std::mem::take(&mut group.coords);
        if group.is_leader {
            self.outcomes.insert(
                group.style_layer_id.clone(),
                (coords.clone(), progress.outcomes),
            );
        }
        let layer_id = StyleLayerId::intern(&group.style_layer_id);
        for (c, meta) in coords.into_iter().zip(progress.metadata) {
            self.pending_uploads.push_back((layer_id, c, meta));
        }
    }

    fn finish_mirrored_and_upload(
        &mut self,
        mut ctx: EpochStepContext<'_>,
        started: instant::Instant,
        budget: Duration,
        mut did_work: bool,
    ) -> bool {
        while !self.mirrored.is_empty() {
            if ctx.sliced && did_work && started.elapsed() >= budget {
                return false;
            }
            let Some(entry) = self.mirrored.pop_front() else {
                break;
            };
            did_work |= self.process_mirrored_group(ctx.world, ctx.style, entry);
        }

        if let Some(Initialized(pool)) = ctx.world.resources.get::<Eventually<SymbolBufferPool>>() {
            // Defer before flushing so GPU updates across all tiles commit atomically in one frame.
            if ctx.sliced
                && did_work
                && !self.pending_uploads.is_empty()
                && started.elapsed() >= budget
            {
                return false;
            }
            self.flush_pending_uploads(pool, &mut ctx);
        } else {
            self.recycle_pending_uploads(ctx.scratch);
        }
        true
    }

    fn process_mirrored_group(
        &mut self,
        world: &World,
        style: &Style,
        (leader_id, follower_id, follower_coords): (String, String, Vec<WorldTileCoords>),
    ) -> bool {
        let (Some((leader_coords, outcome)), Some(paint)) = (
            self.outcomes.get(&leader_id),
            resolve_paint(style, &follower_id),
        ) else {
            return false;
        };
        let leader_refs: Vec<&SymbolLayerData> = leader_coords
            .iter()
            .filter_map(|&c| resolve_layer(world, c, &leader_id))
            .collect();
        if leader_refs.len() != leader_coords.len() {
            return false;
        }
        let mut live_follower_coords = Vec::with_capacity(follower_coords.len());
        let mut follower_refs = Vec::with_capacity(follower_coords.len());
        for c in follower_coords {
            if let Some(layer) = resolve_layer(world, c, &follower_id) {
                live_follower_coords.push(c);
                follower_refs.push(layer);
            }
        }
        let metadata = family::mirror(
            (&leader_refs, outcome),
            &follower_refs,
            (paint, self.zoom),
            &mut self.placed,
        );
        let layer_id = StyleLayerId::intern(&follower_id);
        for (c, meta) in live_follower_coords.into_iter().zip(metadata) {
            self.pending_uploads.push_back((layer_id, c, meta));
        }
        true
    }

    fn flush_pending_uploads(&mut self, pool: &SymbolBufferPool, ctx: &mut EpochStepContext<'_>) {
        while let Some((layer_id, coords, metadata)) = self.pending_uploads.pop_front() {
            let Some(entry) = pool.index().get_layers(coords).and_then(|entries| {
                entries
                    .iter()
                    .find(|entry| entry.style_layer_id == layer_id)
            }) else {
                ctx.scratch.recycle(metadata);
                continue;
            };
            let key = (coords, layer_id);
            let prev = ctx.uploaded.remove(&key);
            let next_cached = upload_diff::diff_and_upload_bucket_with_scratch(
                pool,
                ctx.queue,
                entry,
                prev.as_ref(),
                metadata,
                ctx.scratch,
            );
            if let Some(prev) = prev {
                ctx.scratch.recycle(prev.quads);
            }
            ctx.uploaded.insert(key, next_cached);
        }
    }

    pub(super) fn take_placed(&mut self) -> PlacedSymbols {
        std::mem::take(&mut self.placed)
    }

    pub(super) fn take_grid(&mut self) -> CollisionGrid {
        std::mem::take(&mut self.boxes)
    }

    pub(super) fn abort(mut self, scratch: &mut upload_diff::MetadataScratchPool) -> CollisionGrid {
        if let Some(group) = self.active_group.take() {
            group.recycle(scratch);
        }
        self.recycle_pending_uploads(scratch);
        std::mem::take(&mut self.boxes)
    }

    fn recycle_pending_uploads(&mut self, scratch: &mut upload_diff::MetadataScratchPool) {
        while let Some((_, _, metadata)) = self.pending_uploads.pop_front() {
            scratch.recycle(metadata);
        }
    }

    #[cfg(all(test, feature = "headless", feature = "thread-safe-futures"))]
    pub(super) fn for_atomic_upload_test(
        view_state: ViewState,
        pending_uploads: Vec<(StyleLayerId, WorldTileCoords, Vec<SDFShaderFeatureMetadata>)>,
    ) -> Self {
        let mut outcomes = HashMap::default();
        outcomes.insert("leader".to_owned(), (Vec::new(), Vec::new()));
        let mut mirrored = VecDeque::new();
        mirrored.push_back(("leader".to_owned(), "follower".to_owned(), Vec::new()));
        Self {
            view_state,
            projection: bytemuck::Zeroable::zeroed(),
            zoom: 12.0,
            bearing: 0.0,
            has_symbols: true,
            saw_background_change: false,
            boxes: CollisionGrid::new(0.0, 0.0),
            placed: PlacedSymbols::default(),
            groups: Vec::new(),
            group_idx: 0,
            active_group: None,
            outcomes,
            mirrored,
            pending_uploads: pending_uploads.into_iter().collect(),
        }
    }
}

fn resolve_layer<'a>(
    world: &'a World,
    coords: WorldTileCoords,
    style_layer_id: &str,
) -> Option<&'a SymbolLayerData> {
    world
        .tiles
        .query::<&SymbolLayersDataComponent>(coords)?
        .layers
        .iter()
        .find(|layer| layer.style_layer_id == style_layer_id)
}

fn resolve_paint<'a>(style: &'a Style, style_layer_id: &str) -> Option<&'a SymbolPaint> {
    style.layers.iter().find_map(|layer| {
        if layer.id == style_layer_id {
            if let Some(LayerPaint::Symbol(paint)) = &layer.paint {
                return Some(paint);
            }
        }
        None
    })
}
