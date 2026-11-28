//! The frame memo's key: everything a generated pattern depends on.

use std::hash::{Hash, Hasher};

use rustc_hash::FxHasher;

use crate::{
    coords::{ViewRegion, WorldTileCoords},
    io::tile_sources::RasterSourceId,
};

/// Pool/epoch snapshot supplied by `TileViewPatternSystem` without `render` importing domains.
#[derive(Clone, Copy, Default)]
pub(crate) struct PatternEpochSnapshot {
    pub has_active_cross_fade: bool,
    pub vector_pool_rev: u64,
    pub vector_pool_eviction_gen: u64,
    pub vector_pool_has_capacity: bool,
    pub vector_arrival_epoch: u64,
    pub vector_upload_stamp_epoch: u64,
    pub vector_tiles_len: usize,
    pub raster_tiles_len: usize,
    pub raster_mutation_gen: u64,
}

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

#[derive(Clone)]
pub(crate) struct FastPatternKey {
    tiles_arc: std::sync::Arc<[WorldTileCoords]>,
    style_rev: u64,
    raster_coverings_digest: u64,
    raster_coverings_len: usize,
    vector_pool_rev: u64,
    vector_pool_eviction_gen: u64,
    vector_pool_has_capacity: bool,
    vector_arrival_epoch: u64,
    vector_upload_stamp_epoch: u64,
    tiles_len: (usize, usize),
    raster_mutation_gen: u64,
}

impl FastPatternKey {
    pub(super) fn matches(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.tiles_arc, &other.tiles_arc)
            && self.style_rev == other.style_rev
            && self.raster_coverings_digest == other.raster_coverings_digest
            && self.raster_coverings_len == other.raster_coverings_len
            && self.vector_pool_rev == other.vector_pool_rev
            && self.vector_pool_eviction_gen == other.vector_pool_eviction_gen
            && self.vector_pool_has_capacity == other.vector_pool_has_capacity
            && self.vector_arrival_epoch == other.vector_arrival_epoch
            && self.vector_upload_stamp_epoch == other.vector_upload_stamp_epoch
            && self.tiles_len == other.tiles_len
            && self.raster_mutation_gen == other.raster_mutation_gen
    }
}

/// Computes an O(1) frame memo key backed by `Arc::ptr_eq` on `tiles_arc` and epoch counters,
/// avoiding per-tile `ViewRegion` iteration and world queries during steady/sub-tile motion.
pub(crate) fn compute_fast_memo_key(
    tiles_arc: &std::sync::Arc<[WorldTileCoords]>,
    raster_coverings: &[(RasterSourceId, Vec<WorldTileCoords>)],
    style_rev: u64,
    snapshot: PatternEpochSnapshot,
) -> Option<FastPatternKey> {
    if snapshot.has_active_cross_fade {
        return None;
    }
    let raster_coverings_digest = if raster_coverings.is_empty() {
        0
    } else {
        let mut acc = 0_u64;
        for (source, tiles) in raster_coverings {
            let mut tiles_acc = tiles.len() as u64;
            for coords in tiles {
                tiles_acc = tiles_acc.wrapping_add(digest_of(coords));
            }
            acc = acc.wrapping_add(digest_of(&(source, tiles_acc)));
        }
        acc
    };
    Some(FastPatternKey {
        tiles_arc: std::sync::Arc::clone(tiles_arc),
        style_rev,
        raster_coverings_digest,
        raster_coverings_len: raster_coverings.len(),
        vector_pool_rev: snapshot.vector_pool_rev,
        vector_pool_eviction_gen: snapshot.vector_pool_eviction_gen,
        vector_pool_has_capacity: snapshot.vector_pool_has_capacity,
        vector_arrival_epoch: snapshot.vector_arrival_epoch,
        vector_upload_stamp_epoch: snapshot.vector_upload_stamp_epoch,
        tiles_len: (snapshot.vector_tiles_len, snapshot.raster_tiles_len),
        raster_mutation_gen: snapshot.raster_mutation_gen,
    })
}

/// Computes the frame memo key for `(view_region, raster_coverings, style_rev, snapshot)`, or
pub(crate) type TileVectorStamp = Option<(bool, bool, Option<(u64, u64)>)>;
type TileVectorStampFn<'a> = &'a dyn Fn(WorldTileCoords) -> TileVectorStamp;

/// `None` when active raster cross-fades prevent key reuse.
pub(crate) fn compute_memo_key(
    view_region: &ViewRegion,
    raster_coverings: &[(RasterSourceId, Vec<WorldTileCoords>)],
    style_rev: u64,
    snapshot: PatternEpochSnapshot,
    tile_vector_stamp: TileVectorStampFn<'_>,
) -> Option<PatternMemoKey> {
    (!snapshot.has_active_cross_fade).then(|| {
        build_key(
            view_region,
            raster_coverings,
            style_rev,
            snapshot,
            tile_vector_stamp,
        )
    })
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
    snapshot: PatternEpochSnapshot,
    tile_vector_stamp: TileVectorStampFn<'_>,
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
            if let Some(stamp) = tile_vector_stamp(canonical) {
                view_vector_digest =
                    view_vector_digest.wrapping_add(digest_of(&(canonical, stamp)));
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
    PatternMemoKey {
        style_rev,
        view_region_digest,
        view_region_order_digest: view_region_order.finish(),
        view_region_len,
        raster_coverings_digest,
        raster_coverings_len: raster_coverings.len(),
        vector_pool_rev: snapshot.vector_pool_rev,
        vector_pool_eviction_gen: snapshot.vector_pool_eviction_gen,
        vector_pool_has_capacity: snapshot.vector_pool_has_capacity,
        vector_arrival_epoch: snapshot.vector_arrival_epoch,
        view_vector_digest,
        tiles_len: (snapshot.vector_tiles_len, snapshot.raster_tiles_len),
        raster_mutation_gen: snapshot.raster_mutation_gen,
    }
}
