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

pub(super) type LooseRasterKey = ([[u64; 4]; 4], (u64, u64, usize), u64);

#[derive(Clone, PartialEq, Eq)]
pub(super) struct FrameTerrainDigests {
    pub(super) host_frame: u64,
    pub(super) specs_ptr: usize,
    pub(super) vector_pool_state: (u64, u64, bool),
    pub(super) vector_arrival_epoch: u64,
    pub(super) target_tiles_digest: u64,
    pub(super) relevant_tiles_len: (usize, usize),
    pub(super) raster_gen: u64,
    pub(super) style_rev: u64,
    pub(super) style_vis_digest: u64,
}

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
    pub(super) relevant_tiles_len: (usize, usize),
    pub(super) raster_gen: u64,
    pub(super) style_rev: u64,
    pub(super) style_vis_digest: u64,
}

/// Compares two covering slices as sets so orbit-only reordering never invalidates cached specs
/// or drape retain tracking.
pub(in crate::terrain) fn same_tile_set(a: &[WorldTileCoords], b: &[WorldTileCoords]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    if a == b {
        return true;
    }
    a.iter().all(|t| b.contains(t)) && b.iter().all(|t| a.contains(t))
}

impl TargetSpecsKey {
    pub(super) fn matches_slices(
        &self,
        tiles: &[WorldTileCoords],
        prefetch_fresh: &[WorldTileCoords],
        (arrivals_over_standin, uniform_globe, raster_coverings_digest, loose_raster_digest): (
            bool,
            bool,
            u64,
            u64,
        ),
        digests: &FrameTerrainDigests,
    ) -> bool {
        self.arrivals_over_standin == arrivals_over_standin
            && self.uniform_globe == uniform_globe
            && self.raster_coverings_digest == raster_coverings_digest
            && self.loose_raster_digest == loose_raster_digest
            && self.vector_pool_state == digests.vector_pool_state
            && self.vector_arrival_epoch == digests.vector_arrival_epoch
            && self.target_tiles_digest == digests.target_tiles_digest
            && self.relevant_tiles_len == digests.relevant_tiles_len
            && self.raster_gen == digests.raster_gen
            && self.style_rev == digests.style_rev
            && self.style_vis_digest == digests.style_vis_digest
            && same_tile_set(&self.tiles, tiles)
            && same_tile_set(&self.prefetch_fresh, prefetch_fresh)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct DrapePrepKey {
    pub(super) specs_gen: u64,
    pub(super) drape_paint_zoom_bits: u64,
    pub(super) clear_color_bits: [u64; 4],
    pub(super) strict: bool,
    pub(super) vector_pool_state: (u64, u64, bool),
    pub(super) vector_arrival_epoch: u64,
    pub(super) target_tiles_digest: u64,
    pub(super) relevant_tiles_len: (usize, usize),
    pub(super) raster_gen: u64,
    pub(super) hillshade_fingerprint: u64,
    pub(super) style_rev: u64,
    pub(super) style_vis_digest: u64,
}

pub(super) struct CachedPrep {
    pub(super) prints: Vec<u64>,
    pub(super) ready: Vec<bool>,
    pub(super) needed_shapes: usize,
    pub(super) clipped_at_capacity: Option<usize>,
}

#[derive(Default)]
pub(super) struct DrapePrepareCache {
    specs_key: Option<TargetSpecsKey>,
    specs_gen: u64,
    specs: Option<Arc<Vec<TargetSpec>>>,
    prefetch_drapes: FxHashSet<WorldTileCoords>,
    prep_key: Option<DrapePrepKey>,
    prep: Option<Arc<CachedPrep>>,
    last_digests: Option<FrameTerrainDigests>,
    loose_raster_key: Option<LooseRasterKey>,
    loose_raster: Option<crate::render::eye_covering::RasterCoverings>,
    style_vis_cache: Option<(u64, u64, u64)>,
    draw_after_layer_cache: Option<(u64, u32)>,
}

impl DrapePrepareCache {
    pub(super) fn get_specs_matching(
        &self,
        tiles: &[WorldTileCoords],
        prefetch_fresh: &[WorldTileCoords],
        scalars: (bool, bool, u64, u64),
        digests: &FrameTerrainDigests,
    ) -> Option<(Arc<Vec<TargetSpec>>, &FxHashSet<WorldTileCoords>)> {
        let key = self.specs_key.as_ref()?;
        if key.matches_slices(tiles, prefetch_fresh, scalars, digests) {
            return self
                .specs
                .as_ref()
                .map(|specs| (Arc::clone(specs), &self.prefetch_drapes));
        }
        None
    }

    pub(super) fn store_frame_digests(&mut self, digests: FrameTerrainDigests) {
        self.last_digests = Some(digests);
    }

    pub(super) fn matching_digests(
        &self,
        host_frame: u64,
        specs: &[TargetSpec],
        style_rev: u64,
    ) -> Option<&FrameTerrainDigests> {
        self.last_digests.as_ref().filter(|d| {
            d.host_frame == host_frame
                && d.style_rev == style_rev
                && d.specs_ptr == specs.as_ptr() as usize
        })
    }

    pub(super) fn get_loose_raster(
        &self,
        key: LooseRasterKey,
    ) -> Option<crate::render::eye_covering::RasterCoverings> {
        (self.loose_raster_key == Some(key))
            .then(|| self.loose_raster.clone())
            .flatten()
    }

    pub(super) fn store_loose_raster(
        &mut self,
        key: LooseRasterKey,
        raster: crate::render::eye_covering::RasterCoverings,
    ) {
        self.loose_raster_key = Some(key);
        self.loose_raster = Some(raster);
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

    pub(super) fn get_prep(&self, key: &DrapePrepKey, capacity: usize) -> Option<Arc<CachedPrep>> {
        if self.prep_key.as_ref() != Some(key) {
            return None;
        }
        self.prep
            .as_ref()
            .filter(|prep| {
                prep.clipped_at_capacity
                    .map_or(capacity >= prep.needed_shapes, |cap| cap == capacity)
            })
            .map(Arc::clone)
    }

    pub(super) fn store_prep(&mut self, key: DrapePrepKey, prep: Arc<CachedPrep>) {
        self.prep_key = Some(key);
        self.prep = Some(prep);
    }

    pub(super) fn cached_draw_after_layer_index(&mut self, style: &Style) -> u32 {
        let rev = style.revision();
        if let Some((cached_rev, index)) = self.draw_after_layer_cache
            && cached_rev == rev
        {
            return index;
        }
        let index = style
            .layers
            .iter()
            .filter(|layer| layer.type_ == "background")
            .map(|layer| layer.index)
            .max()
            .unwrap_or(0);
        self.draw_after_layer_cache = Some((rev, index));
        index
    }

    pub(super) fn style_visibility_digest_cached(&mut self, style: &Style, zoom: f64) -> u64 {
        let key = (style.revision(), zoom.to_bits());
        if let Some((rev, zoom_bits, digest)) = self.style_vis_cache
            && rev == key.0
            && zoom_bits == key.1
        {
            return digest;
        }
        let digest = style_visibility_digest(style, zoom);
        self.style_vis_cache = Some((key.0, key.1, digest));
        digest
    }
}

pub(super) fn relevant_tiles_len(world: &World) -> (usize, usize) {
    let vector = world.tiles.pyramid_of::<VectorLayerBucketComponent>().len();
    #[cfg(feature = "raster")]
    let raster = world
        .tiles
        .pyramid_of::<crate::raster::RasterLayersDataComponent>()
        .len();
    #[cfg(not(feature = "raster"))]
    let raster = 0;
    (vector, raster)
}

pub(super) fn raster_coverings_digest(coverings: &[(RasterSourceId, Vec<WorldTileCoords>)]) -> u64 {
    coverings
        .iter()
        .fold(coverings.len() as u64, |acc, (source, tiles)| {
            let mut sh = FxHasher::default();
            source.hash(&mut sh);
            let tile_digest = tiles.iter().fold(tiles.len() as u64, |t_acc, coords| {
                let mut th = FxHasher::default();
                coords.hash(&mut th);
                t_acc.wrapping_add(th.finish())
            });
            tile_digest.hash(&mut sh);
            acc.wrapping_add(sh.finish())
        })
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
