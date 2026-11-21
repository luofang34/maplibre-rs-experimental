//! Frame-to-frame memoization for [`TileViewPattern::generate_pattern`] during sub-tile camera
//! motion where the visible covering, GPU pools, and loaded tile states did not change.

use std::hash::{Hash, Hasher};

use rustc_hash::FxHasher;

use super::TileViewPattern;
use crate::{
    coords::{ViewRegion, WorldTileCoords, Zoom},
    io::tile_sources::RasterSourceId,
    render::eventually::{Eventually, Eventually::Initialized},
    tcs::world::World,
    vector::{VectorBufferPool, VectorLayerBucketComponent},
};

#[derive(Clone, PartialEq, Eq)]
struct PatternMemoKey {
    style_rev: u64,
    view_region_digest: u64,
    view_region_len: usize,
    raster_coverings_digest: u64,
    raster_coverings_len: usize,
    vector_pool_rev: u64,
    vector_pool_eviction_gen: u64,
    vector_pool_has_capacity: bool,
    vector_arrival_epoch: u64,
    view_vector_digest: u64,
    tiles_len: usize,
    raster_mutation_gen: u64,
}

/// Caches the most recent [`TileViewPattern`] key across frames so smooth panning or zooming
/// inside an unchanged covering only updates per-shape zoom transforms in place.
#[derive(Default)]
pub struct TileViewPatternMemo {
    key: Option<PatternMemoKey>,
    last_zoom_bits: u64,
}

impl TileViewPatternMemo {
    /// Returns `true` if `tile_view_pattern` already holds the exact covering/source selection
    /// for `(view_region, raster_coverings, style_rev, world)`.
    pub(crate) fn matches(
        &mut self,
        view_region: &ViewRegion,
        raster_coverings: &[(RasterSourceId, Vec<WorldTileCoords>)],
        style_rev: u64,
        world: &World,
    ) -> bool {
        if has_active_cross_fade(world) {
            self.invalidate();
            return false;
        }
        let next_key = build_key(view_region, raster_coverings, style_rev, world);
        if self.key.as_ref() == Some(&next_key) {
            return true;
        }
        self.invalidate();
        false
    }

    /// Returns `true` if `zoom` changed since the previous frame and records the new zoom.
    pub(crate) fn needs_zoom_update(&mut self, zoom: Zoom) -> bool {
        let zoom_bits = zoom.value().to_bits();
        if self.last_zoom_bits != zoom_bits {
            self.last_zoom_bits = zoom_bits;
            true
        } else {
            false
        }
    }

    pub(crate) fn record(
        &mut self,
        view_region: &ViewRegion,
        raster_coverings: &[(RasterSourceId, Vec<WorldTileCoords>)],
        style_rev: u64,
        zoom: Zoom,
        world: &World,
    ) {
        if has_active_cross_fade(world) {
            self.invalidate();
            return;
        }
        self.key = Some(build_key(view_region, raster_coverings, style_rev, world));
        self.last_zoom_bits = zoom.value().to_bits();
    }

    fn invalidate(&mut self) {
        self.key = None;
    }
}

impl<Q, B> TileViewPattern<Q, B> {
    pub(crate) fn is_empty(&self) -> bool {
        self.view_tiles.is_empty()
    }

    pub(crate) fn update_zoom_in_place(&mut self, zoom: Zoom) {
        for view_tile in &mut self.view_tiles {
            view_tile
                .vector
                .for_each_mut(&mut |shape| shape.set_zoom(zoom));
            for (_, shapes) in &mut view_tile.raster {
                shapes.for_each_mut(&mut |shape| shape.set_zoom(zoom));
            }
        }
    }
}

fn has_active_cross_fade(world: &World) -> bool {
    #[cfg(feature = "raster")]
    {
        world
            .resources
            .get::<crate::raster::cross_fade::RasterCrossFade>()
            .is_some_and(|fade| !fade.is_empty())
    }
    #[cfg(not(feature = "raster"))]
    {
        let _ = world;
        false
    }
}

fn build_key(
    view_region: &ViewRegion,
    raster_coverings: &[(RasterSourceId, Vec<WorldTileCoords>)],
    style_rev: u64,
    world: &World,
) -> PatternMemoKey {
    let mut vh = FxHasher::default();
    let mut view_region_len = 0_usize;
    let mut tv = FxHasher::default();
    for seen in view_region.copies() {
        view_region_len += 1;
        seen.hash(&mut vh);
        if let Some((canonical, _)) = seen.wrapped() {
            if let Some(buckets) = world.tiles.query::<&VectorLayerBucketComponent>(canonical) {
                (
                    canonical,
                    buckets.done,
                    buckets.failed,
                    buckets.uploaded_stamp,
                )
                    .hash(&mut tv);
            }
        }
    }
    let mut rh = FxHasher::default();
    for (source, tiles) in raster_coverings {
        source.hash(&mut rh);
        tiles.len().hash(&mut rh);
        for coords in tiles {
            coords.hash(&mut rh);
        }
    }
    let (vector_pool_rev, vector_pool_eviction_gen, vector_pool_has_capacity) =
        match world.resources.get::<Eventually<VectorBufferPool>>() {
            Some(Initialized(pool)) => (
                pool.revision(),
                pool.eviction_generation(),
                pool.has_capacity(),
            ),
            _ => (0, 0, false),
        };
    #[cfg(feature = "raster")]
    let raster_mutation_gen = match world
        .resources
        .get::<Eventually<crate::raster::resource::RasterResources>>()
    {
        Some(Initialized(raster)) => raster.mutation_generation(),
        _ => 0,
    };
    #[cfg(not(feature = "raster"))]
    let raster_mutation_gen = 0_u64;

    PatternMemoKey {
        style_rev,
        view_region_digest: vh.finish(),
        view_region_len,
        raster_coverings_digest: rh.finish(),
        raster_coverings_len: raster_coverings.len(),
        vector_pool_rev,
        vector_pool_eviction_gen,
        vector_pool_has_capacity,
        vector_arrival_epoch: crate::vector::arrival_epoch(world),
        view_vector_digest: tv.finish(),
        tiles_len: world.tiles.tiles.len(),
        raster_mutation_gen,
    }
}
