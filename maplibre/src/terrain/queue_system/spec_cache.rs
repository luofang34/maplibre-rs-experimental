//! Cross-frame memoization for terrain `TargetSpec` construction, drape fingerprints, and
//! source readiness checks on steady camera / panning frames.

use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

use rustc_hash::{FxHashSet, FxHasher};

use crate::{
    coords::WorldTileCoords,
    io::tile_sources::RasterSourceId,
    render::eventually::{Eventually, Eventually::Initialized},
    style::Style,
    tcs::world::World,
    terrain::drape_targets::TargetSpec,
    vector::{VectorBufferPool, VectorLayerBucketComponent},
};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct TargetSpecsKey {
    pub(super) tiles: Vec<WorldTileCoords>,
    pub(super) prefetch_fresh: Vec<WorldTileCoords>,
    pub(super) arrivals_over_standin: bool,
    pub(super) uniform_globe: bool,
    pub(super) raster_coverings_digest: u64,
    pub(super) loose_raster_digest: u64,
    pub(super) vector_pool_state: (u64, u64, bool),
    pub(super) vector_arrival_epoch: u64,
    pub(super) target_tiles_digest: u64,
    pub(super) tiles_len: usize,
    pub(super) raster_gen: u64,
    pub(super) style_rev: u64,
    pub(super) style_vis_digest: u64,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct DrapePrepKey {
    pub(super) specs_gen: u64,
    pub(super) drape_paint_zoom_bits: u64,
    pub(super) clear_color_bits: [u64; 4],
    pub(super) strict: bool,
    pub(super) capacity: usize,
    pub(super) vector_pool_state: (u64, u64, bool),
    pub(super) vector_arrival_epoch: u64,
    pub(super) target_tiles_digest: u64,
    pub(super) tiles_len: usize,
    pub(super) raster_gen: u64,
    pub(super) hillshade_fingerprint: u64,
    pub(super) style_rev: u64,
    pub(super) style_vis_digest: u64,
}

pub(super) struct CachedPrep {
    pub(super) prints: Vec<u64>,
    pub(super) ready: Vec<bool>,
}

#[derive(Default)]
pub(super) struct DrapePrepareCache {
    specs_key: Option<TargetSpecsKey>,
    specs_gen: u64,
    specs: Option<Arc<Vec<TargetSpec>>>,
    prefetch_drapes: FxHashSet<WorldTileCoords>,
    prep_key: Option<DrapePrepKey>,
    prep: Option<Arc<CachedPrep>>,
}

impl DrapePrepareCache {
    pub(super) fn get_specs(
        &self,
        key: &TargetSpecsKey,
    ) -> Option<(Arc<Vec<TargetSpec>>, &FxHashSet<WorldTileCoords>)> {
        if self.specs_key.as_ref() == Some(key) {
            return self
                .specs
                .as_ref()
                .map(|specs| (Arc::clone(specs), &self.prefetch_drapes));
        }
        None
    }

    pub(super) fn store_specs(
        &mut self,
        key: TargetSpecsKey,
        specs: Arc<Vec<TargetSpec>>,
        prefetch_drapes: FxHashSet<WorldTileCoords>,
    ) {
        self.specs_gen = self.specs_gen.wrapping_add(1);
        self.specs_key = Some(key);
        self.specs = Some(specs);
        self.prefetch_drapes = prefetch_drapes;
        self.prep_key = None;
        self.prep = None;
    }

    pub(super) fn specs_gen_for(&self, specs: &[TargetSpec]) -> Option<u64> {
        self.specs
            .as_ref()
            .filter(|cached| std::ptr::eq(cached.as_slice(), specs))
            .map(|_| self.specs_gen)
    }

    pub(super) fn get_prep(&self, key: &DrapePrepKey) -> Option<Arc<CachedPrep>> {
        if self.prep_key.as_ref() == Some(key) {
            return self.prep.as_ref().map(Arc::clone);
        }
        None
    }

    pub(super) fn store_prep(&mut self, key: DrapePrepKey, prep: Arc<CachedPrep>) {
        self.prep_key = Some(key);
        self.prep = Some(prep);
    }
}

pub(super) fn raster_coverings_digest(coverings: &[(RasterSourceId, Vec<WorldTileCoords>)]) -> u64 {
    let mut h = FxHasher::default();
    coverings.len().hash(&mut h);
    for (source, tiles) in coverings {
        source.hash(&mut h);
        tiles.len().hash(&mut h);
        for coords in tiles {
            coords.hash(&mut h);
        }
    }
    h.finish()
}

pub(super) fn style_visibility_digest(style: &Style, zoom: f64) -> u64 {
    let mut h = FxHasher::default();
    style.layers.len().hash(&mut h);
    for layer in &style.layers {
        layer.is_visible_at(zoom).hash(&mut h);
    }
    h.finish()
}

pub(super) fn vector_pool_state(world: &World) -> (u64, u64, bool) {
    match world.resources.get::<Eventually<VectorBufferPool>>() {
        Some(Initialized(pool)) => (
            pool.revision(),
            pool.eviction_generation(),
            pool.has_capacity(),
        ),
        _ => (0, 0, false),
    }
}

pub(super) fn raster_mutation_gen(world: &World) -> u64 {
    #[cfg(feature = "raster")]
    {
        match world
            .resources
            .get::<Eventually<crate::raster::resource::RasterResources>>()
        {
            Some(Initialized(raster)) => raster.mutation_generation(),
            _ => 0,
        }
    }
    #[cfg(not(feature = "raster"))]
    {
        let _ = world;
        0
    }
}

pub(super) fn hillshade_fingerprint(world: &World) -> u64 {
    match world
        .resources
        .get::<Eventually<crate::hillshade::HillshadeResources>>()
    {
        Some(Initialized(hillshade)) => hillshade.fingerprint_all(),
        _ => 0,
    }
}

pub(super) fn target_coords_vector_digest(
    coords_iter: impl Iterator<Item = WorldTileCoords>,
    world: &World,
) -> u64 {
    let mut h = FxHasher::default();
    for coords in coords_iter {
        if let Some(buckets) = world.tiles.query::<&VectorLayerBucketComponent>(coords) {
            (
                coords,
                buckets.done,
                buckets.failed,
                buckets.layers.len(),
                buckets.uploaded_stamp,
            )
                .hash(&mut h);
        }
    }
    h.finish()
}
