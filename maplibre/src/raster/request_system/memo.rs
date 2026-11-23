//! Per-frame memoization and scratch storage for raster and DEM-layer tile requests.

use std::hash::{Hash, Hasher};

use rustc_hash::{FxHashMap, FxHashSet, FxHasher};

use crate::{
    coords::WorldTileCoords,
    io::{
        tile_backpressure::has_raster_deferred,
        tile_retry::{self, RequestKind},
        tile_sources::{source_layer_groups, RasterSourceId, TileKind},
    },
    render::{
        eye_covering::{
            frame_view_proj_bits, terrain_index_key, FrameLooseCovering, MultiViewExtractCovering,
            RasterCoverings,
        },
        frame_signals::host_frame_in_progress,
        view_state::{ViewState, ViewStatePadding},
    },
    style::Style,
    tcs::world::World,
    terrain::request_system::DrapeRequests,
};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct RasterRequestKey {
    style_rev: u64,
    view_proj_bits: [[u64; 4]; 4],
    viewport_bits: (u64, u64),
    zoom_bits: u64,
    style_zoom_bits: u64,
    overscan_bits: u64,
    external_or_pose: (bool, bool),
    tight_padding: bool,
    terrain_key: (u64, u64, usize),
    drapes_hash: u64,
    raster_arrival_epoch: u64,
    dem_arrival_epoch: u64,
    tiles_len: usize,
    multi_key: Option<u64>,
}

#[derive(Default)]
pub(super) struct WantedTilesAcc {
    pub(super) stand_ins: Vec<WorldTileCoords>,
    pub(super) seen_stand_ins: FxHashSet<WorldTileCoords>,
    pub(super) wanted: Vec<WorldTileCoords>,
    pub(super) coarse_ancestors: Vec<WorldTileCoords>,
    pub(super) seen_coarse: FxHashSet<WorldTileCoords>,
    pub(super) needed_coarse: FxHashSet<WorldTileCoords>,
    pub(super) sources_by_coord: FxHashMap<WorldTileCoords, Vec<RasterSourceId>>,
}

impl WantedTilesAcc {
    pub(super) fn reset(&mut self) {
        self.stand_ins.clear();
        self.seen_stand_ins.clear();
        self.wanted.clear();
        self.coarse_ancestors.clear();
        self.seen_coarse.clear();
        self.needed_coarse.clear();
        self.sources_by_coord.clear();
    }
}

#[derive(Default)]
pub(super) struct RasterRequestMemo {
    style_cache: Option<(u64, bool, Vec<RasterSourceId>)>,
    last_key: Option<RasterRequestKey>,
    cached_regions: Option<(RasterCoverings, ViewStatePadding, (u64, u64, usize))>,
    pub(super) acc: WantedTilesAcc,
    pub(super) requested_scratch: FxHashSet<WorldTileCoords>,
}

fn hash_drapes(coords: &[WorldTileCoords]) -> u64 {
    let mut hasher = FxHasher::default();
    coords.len().hash(&mut hasher);
    for c in coords {
        c.hash(&mut hasher);
    }
    hasher.finish()
}

fn hash_multi_raster(multi: &MultiViewExtractCovering) -> u64 {
    let mut hasher = FxHasher::default();
    multi.raster_regions.len().hash(&mut hasher);
    for (src, tiles) in &multi.raster_regions {
        src.hash(&mut hasher);
        hash_drapes(tiles).hash(&mut hasher);
    }
    multi.skipped_wanted_raster.len().hash(&mut hasher);
    let mut skipped_mix = 0_u64;
    for c in &multi.skipped_wanted_raster {
        let mut elem = FxHasher::default();
        c.hash(&mut elem);
        skipped_mix ^= elem.finish();
    }
    skipped_mix.hash(&mut hasher);
    hasher.finish()
}

impl RasterRequestMemo {
    pub(super) fn sync_style(&mut self, style: &Style) -> (bool, bool) {
        let style_rev = style.revision();
        let valid = self
            .style_cache
            .as_ref()
            .is_some_and(|(rev, _, _)| *rev == style_rev);
        if !valid {
            let has_raster_layers = style.layers.iter().any(|layer| {
                crate::io::tile_sources::RASTER_LAYER_TYPES.contains(&layer.type_.as_str())
                    && !layer.is_hidden()
            });
            let all_sources = if has_raster_layers {
                source_layer_groups(style, TileKind::Raster)
                    .into_iter()
                    .filter(|group| {
                        !crate::raster::media_source::is_media_source(
                            style,
                            group.source_name.as_deref(),
                        )
                    })
                    .map(|group| RasterSourceId::new(group.source_name))
                    .collect()
            } else {
                Vec::new()
            };
            self.style_cache = Some((style_rev, has_raster_layers, all_sources));
        }
        self.style_cache
            .as_ref()
            .map_or((false, true), |(_, has_layers, sources)| {
                (*has_layers, sources.is_empty())
            })
    }

    pub(super) fn cached_sources(&self) -> Vec<RasterSourceId> {
        self.style_cache
            .as_ref()
            .map(|(_, _, sources)| sources.clone())
            .unwrap_or_default()
    }

    pub(super) fn build_key(
        world: &World,
        style: &Style,
        view_state: &ViewState,
        tight_padding: bool,
        multi: Option<&MultiViewExtractCovering>,
    ) -> Option<RasterRequestKey> {
        if !view_state.has_valid_perspective() {
            return None;
        }
        let drapes_hash = world
            .resources
            .get::<DrapeRequests>()
            .map_or(0, |r| hash_drapes(&r.0));
        Some(RasterRequestKey {
            style_rev: style.revision(),
            view_proj_bits: frame_view_proj_bits(world, style, view_state),
            viewport_bits: (view_state.width().to_bits(), view_state.height().to_bits()),
            zoom_bits: view_state.zoom().value().to_bits(),
            style_zoom_bits: view_state.style_zoom().value().to_bits(),
            overscan_bits: view_state.request_overscan().to_bits(),
            external_or_pose: (
                view_state.has_external_view(),
                view_state.pose_view().is_some(),
            ),
            tight_padding,
            terrain_key: terrain_index_key(world),
            drapes_hash,
            raster_arrival_epoch: crate::raster::raster_arrival_epoch(world),
            dem_arrival_epoch: crate::terrain::dem_arrival_epoch(world),
            tiles_len: world.tiles.tiles.len(),
            multi_key: multi.map(hash_multi_raster),
        })
    }

    pub(super) fn try_reuse(&self, world: &mut World, key: &RasterRequestKey) -> bool {
        if self.last_key.as_ref() != Some(key)
            || has_raster_deferred(world)
            || tile_retry::kind_has_due_retry(world, RequestKind::Raster)
        {
            return false;
        }
        if key.multi_key.is_none() {
            let Some((regions, padding, terrain_key)) = self.cached_regions.as_ref() else {
                return false;
            };
            let host_frame = host_frame_in_progress(world);
            let cache = world.resources.get_or_init_mut::<FrameLooseCovering>();
            if cache.host_frame != host_frame {
                *cache = FrameLooseCovering::for_host_frame(host_frame);
            }
            if cache
                .raster_regions
                .as_ref()
                .is_none_or(|(_, p, k)| *p != *padding || *k != *terrain_key)
            {
                cache.raster_regions = Some((regions.clone(), *padding, *terrain_key));
            }
        }
        true
    }

    pub(super) fn invalidate(&mut self) {
        self.last_key = None;
    }

    pub(super) fn record(
        &mut self,
        mut key: RasterRequestKey,
        tiles_len: usize,
        regions: Option<(RasterCoverings, ViewStatePadding, (u64, u64, usize))>,
    ) {
        key.tiles_len = tiles_len;
        self.last_key = Some(key);
        if regions.is_some() {
            self.cached_regions = regions;
        }
    }
}
