//! The frame memo's key: everything a generated pattern depends on.

use std::hash::{Hash, Hasher};

use rustc_hash::FxHasher;

use crate::{
    coords::{ViewRegion, WorldTileCoords},
    io::tile_sources::RasterSourceId,
    render::eventually::{Eventually, Eventually::Initialized},
    tcs::world::World,
    vector::{VectorBufferPool, VectorLayerBucketComponent},
};

/// The inputs a generated pattern depends on. The view region enters as a set: a covering is
/// listed nearest the camera's focus first, so an orbit over an unchanged covering reorders it,
/// and a pattern built from one order draws the same tiles as one built from another.
#[derive(Clone)]
pub(crate) struct PatternMemoKey {
    style_rev: u64,
    view_region_digest: u64,
    view_region_order_digest: u64,
    view_region_len: usize,
    raster_coverings_digest: u64,
    raster_coverings_len: usize,
    vector_pool_rev: u64,
    vector_pool_eviction_gen: u64,
    vector_pool_has_capacity: bool,
    vector_arrival_epoch: u64,
    view_vector_digest: u64,
    tiles_len: (usize, usize),
    raster_mutation_gen: u64,
}

impl PatternMemoKey {
    pub(super) fn same_inputs(&self, other: &Self, order_matters: bool) -> bool {
        self.style_rev == other.style_rev
            && self.view_region_digest == other.view_region_digest
            && self.view_region_len == other.view_region_len
            && (!order_matters || self.view_region_order_digest == other.view_region_order_digest)
            && self.raster_coverings_digest == other.raster_coverings_digest
            && self.raster_coverings_len == other.raster_coverings_len
            && self.vector_pool_rev == other.vector_pool_rev
            && self.vector_pool_eviction_gen == other.vector_pool_eviction_gen
            && self.vector_pool_has_capacity == other.vector_pool_has_capacity
            && self.vector_arrival_epoch == other.vector_arrival_epoch
            && self.view_vector_digest == other.view_vector_digest
            && self.tiles_len == other.tiles_len
            && self.raster_mutation_gen == other.raster_mutation_gen
    }
}

/// Computes the frame memo key for `(view_region, raster_coverings, style_rev, world)`, or
/// `None` when active raster cross-fades prevent key reuse.
pub(crate) fn compute_memo_key(
    view_region: &ViewRegion,
    raster_coverings: &[(RasterSourceId, Vec<WorldTileCoords>)],
    style_rev: u64,
    world: &World,
) -> Option<PatternMemoKey> {
    (!has_active_cross_fade(world))
        .then(|| build_key(view_region, raster_coverings, style_rev, world))
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

fn digest_of(value: &impl Hash) -> u64 {
    let mut hasher = FxHasher::default();
    value.hash(&mut hasher);
    hasher.finish()
}

fn build_key(
    view_region: &ViewRegion,
    raster_coverings: &[(RasterSourceId, Vec<WorldTileCoords>)],
    style_rev: u64,
    world: &World,
) -> PatternMemoKey {
    let mut view_region_digest = 0_u64;
    let mut view_region_order = FxHasher::default();
    let mut view_region_len = 0_usize;
    let mut view_vector_digest = 0_u64;
    for seen in view_region.copies() {
        view_region_len += 1;
        seen.hash(&mut view_region_order);
        view_region_digest = view_region_digest.wrapping_add(digest_of(&seen));
        if let Some((canonical, _)) = seen.wrapped() {
            if let Some(buckets) = world.tiles.query::<&VectorLayerBucketComponent>(canonical) {
                view_vector_digest = view_vector_digest.wrapping_add(digest_of(&(
                    canonical,
                    buckets.done,
                    buckets.failed,
                    buckets.uploaded_stamp,
                )));
            }
        }
    }
    let mut raster_coverings_digest = 0_u64;
    for (source, tiles) in raster_coverings {
        let mut tiles_acc = tiles.len() as u64;
        for coords in tiles {
            tiles_acc = tiles_acc.wrapping_add(digest_of(coords));
        }
        raster_coverings_digest =
            raster_coverings_digest.wrapping_add(digest_of(&(source, tiles_acc)));
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

    #[cfg(feature = "raster")]
    let raster_tiles_len = world
        .tiles
        .pyramid_of::<crate::raster::RasterLayersDataComponent>()
        .len();
    #[cfg(not(feature = "raster"))]
    let raster_tiles_len = 0_usize;
    let vector_tiles_len = world.tiles.pyramid_of::<VectorLayerBucketComponent>().len();

    PatternMemoKey {
        style_rev,
        view_region_digest,
        view_region_order_digest: view_region_order.finish(),
        view_region_len,
        raster_coverings_digest,
        raster_coverings_len: raster_coverings.len(),
        vector_pool_rev,
        vector_pool_eviction_gen,
        vector_pool_has_capacity,
        vector_arrival_epoch: crate::vector::arrival_epoch(world),
        view_vector_digest,
        tiles_len: (vector_tiles_len, raster_tiles_len),
        raster_mutation_gen,
    }
}
