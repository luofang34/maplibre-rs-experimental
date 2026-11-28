//! Shared boundary elevations for independently loaded terrain meshes.
use std::{
    collections::HashMap,
    hash::{BuildHasherDefault, Hasher},
    sync::Arc,
};

use rustc_hash::FxHashMap;
use sample::{build_edges, Dems, Sources};
#[cfg(test)]
use sample::{scale_of, shared_height};

use crate::{
    coords::{WorldTileCoords, EXTENT},
    tcs::tiles::Tiles,
    terrain::{
        coverage::EdgeHeights, dem::DemTile, mesh::TERRAIN_MESH_SIZE,
        resources::TerrainTileUniforms, DemRevision, DemTileComponent,
    },
};

mod sample;

const N: usize = TERRAIN_MESH_SIZE as usize;

/// Fast non-cryptographic integer hasher for [`WorldTileCoords`] keys in hot edge loops.
#[derive(Default)]
struct TileHasher(u64);

impl Hasher for TileHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x517c_c1b7_2722_0a95);
        }
    }

    #[inline]
    fn write_i32(&mut self, i: i32) {
        self.write_u64(i as u32 as u64);
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.write_u64(u64::from(i));
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

type FastTileMap<V> = HashMap<WorldTileCoords, V, BuildHasherDefault<TileHasher>>;

type Signature = Vec<(WorldTileCoords, Option<(WorldTileCoords, DemRevision)>)>;

#[derive(Default)]
pub(crate) struct EdgeCache {
    signature: Signature,
    sig_scratch: Signature,
    samples: Arc<FxHashMap<WorldTileCoords, Arc<EdgeHeights>>>,
    row_index: FastTileMap<u32>,
    free_rows: Vec<u32>,
    packed_edges: Vec<EdgeHeights>,
    dirty_rows: Vec<u32>,
    generation: u64,
    last_fast_key: Option<(u64, u64, usize, usize)>,
    /// How many tiles' edges the last change rebuilt.
    pub(crate) rebuilt: usize,
}

fn sources_digest(
    sources: &[(
        Option<WorldTileCoords>,
        WorldTileCoords,
        Option<WorldTileCoords>,
    )],
) -> u64 {
    use std::hash::Hash;
    sources
        .iter()
        .fold(sources.len() as u64, |acc, (dem, tile, _)| {
            let mut hasher = TileHasher::default();
            dem.hash(&mut hasher);
            tile.hash(&mut hasher);
            acc.wrapping_add(hasher.finish())
        })
}

impl EdgeCache {
    fn sync_signature(
        &mut self,
        sources: &[(
            Option<WorldTileCoords>,
            WorldTileCoords,
            Option<WorldTileCoords>,
        )],
        tiles: &Tiles,
        max_rows: usize,
    ) {
        self.sig_scratch.clear();
        self.sig_scratch.reserve(sources.len());
        for (source, tile, _) in sources {
            let revision =
                source.and_then(|source| match tiles.query::<&DemTileComponent>(source) {
                    Some(DemTileComponent::Loaded(dem)) => Some((source, dem.revision_key())),
                    _ => None,
                });
            self.sig_scratch.push((*tile, revision));
        }
        self.sig_scratch
            .sort_by_key(|(tile, _)| (u8::from(tile.z), tile.y, tile.x));
        if self.sig_scratch != self.signature {
            self.update(tiles, max_rows);
        }
    }
}

type EdgeSourceKey = (
    Option<WorldTileCoords>,
    WorldTileCoords,
    Option<WorldTileCoords>,
);

impl EdgeCache {
    #[cfg(test)]
    pub(super) fn packed_edges(&self) -> &[EdgeHeights] {
        &self.packed_edges
    }

    pub(crate) fn assign_and_upload_edges(
        &mut self,
        terrain: &mut crate::terrain::resources::TerrainResources,
        queue: &crate::render::upload_queue::UploadQueue,
        (sources, uniforms): (&[EdgeSourceKey], &mut [TerrainTileUniforms]),
        tiles: &Tiles,
        dem_arrival_epoch: u64,
    ) -> u64 {
        let (edge_gen, fallback_edges) = self.assign_indices_with_epoch(
            sources,
            uniforms,
            tiles,
            terrain.edge_capacity(),
            Some(dem_arrival_epoch),
        );
        let dirty_rows = std::mem::take(&mut self.dirty_rows);
        if let Some(edges) = &fallback_edges {
            terrain.write_edges_if_stale(queue, 0, edges, &[]);
        } else {
            terrain.write_edges_if_stale(queue, edge_gen, &self.packed_edges, &dirty_rows);
        }
        edge_gen
    }

    pub(crate) fn sync_coverage_edges(
        &self,
        index: &mut crate::terrain::TerrainCoverageIndex,
        written_sources: &[(
            Option<WorldTileCoords>,
            WorldTileCoords,
            Option<WorldTileCoords>,
        )],
        edge_gen: u64,
    ) {
        index.set_surface_edges_if_stale(written_sources, &self.samples, edge_gen);
    }

    /// Assigns each tile's `edge_index[0]` into its uniform block and returns `(generation, fallback)`.
    /// When `packed_edges` fits within `max_rows` and matches an unchanged signature, callers skip
    /// copying and uploading `EdgeHeights` to the GPU.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn assign_indices(
        &mut self,
        sources: &[(
            Option<WorldTileCoords>,
            WorldTileCoords,
            Option<WorldTileCoords>,
        )],
        uniforms: &mut [TerrainTileUniforms],
        tiles: &Tiles,
        max_rows: usize,
    ) -> (u64, Option<Vec<EdgeHeights>>) {
        self.assign_indices_with_epoch(sources, uniforms, tiles, max_rows, None)
    }

    pub(crate) fn assign_indices_with_epoch(
        &mut self,
        sources: &[(
            Option<WorldTileCoords>,
            WorldTileCoords,
            Option<WorldTileCoords>,
        )],
        uniforms: &mut [TerrainTileUniforms],
        tiles: &Tiles,
        max_rows: usize,
        dem_arrival_epoch: Option<u64>,
    ) -> (u64, Option<Vec<EdgeHeights>>) {
        let fast_key = dem_arrival_epoch.map(|epoch| {
            (
                sources_digest(sources),
                epoch,
                tiles.pyramid_of::<DemTileComponent>().len(),
                max_rows,
            )
        });
        if fast_key.is_none()
            || self.last_fast_key != fast_key
            || self.packed_edges.len() > max_rows
        {
            self.sync_signature(sources, tiles, max_rows);
            self.last_fast_key = fast_key;
        }
        if self.packed_edges.len() <= max_rows {
            for ((_, tile, _), uniform) in sources.iter().zip(uniforms.iter_mut()) {
                uniform.edge_index[0] = self.row_index.get(tile).copied().unwrap_or(0);
            }
            return (self.generation, None);
        }
        let mut edges = Vec::with_capacity(sources.len());
        for (index, ((_, tile, _), uniform)) in sources.iter().zip(uniforms.iter_mut()).enumerate()
        {
            uniform.edge_index[0] = index as u32;
            edges.push(self.samples.get(tile).map(|e| **e).unwrap_or_default());
        }
        (0, Some(edges))
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn apply(
        &mut self,
        sources: &[(
            Option<WorldTileCoords>,
            WorldTileCoords,
            Option<WorldTileCoords>,
        )],
        uniforms: &mut [TerrainTileUniforms],
        tiles: &Tiles,
    ) -> Vec<EdgeHeights> {
        self.sync_signature(sources, tiles, usize::MAX);
        self.last_fast_key = None;
        let mut edges = Vec::with_capacity(sources.len());
        for (index, ((_, tile, _), uniform)) in sources.iter().zip(uniforms.iter_mut()).enumerate()
        {
            let edge = self.samples.get(tile).map(|e| **e).unwrap_or_default();
            uniform.edge_index[0] = index as u32;
            edges.push(edge);
        }
        edges
    }

    fn compact_rows(&mut self) {
        let samples = &self.samples;
        self.row_index.clear();
        self.free_rows.clear();
        self.packed_edges.clear();
        self.dirty_rows.clear();
        for (tile, _) in &self.sig_scratch {
            if self.row_index.contains_key(tile) {
                continue;
            }
            let row = self.packed_edges.len() as u32;
            self.row_index.insert(*tile, row);
            self.packed_edges
                .push(samples.get(tile).map(|e| **e).unwrap_or_default());
            self.dirty_rows.push(row);
        }
    }

    fn evict_stale_rows(&mut self, after: &FastTileMap<&Option<(WorldTileCoords, DemRevision)>>) {
        let samples = Arc::make_mut(&mut self.samples);
        samples.retain(|tile, _| after.contains_key(tile));
        let free = &mut self.free_rows;
        self.row_index.retain(|tile, &mut row| {
            let keep = after.contains_key(tile);
            if !keep {
                free.push(row);
            }
            keep
        });
    }

    fn write_affected_rows(
        &mut self,
        affected: &[WorldTileCoords],
        selection: &Sources,
        dems: &Dems<'_>,
    ) {
        let samples = Arc::make_mut(&mut self.samples);
        for &tile in affected {
            let edge = build_edges(tile, selection, dems);
            samples.insert(tile, Arc::new(edge));
            let row = if let Some(&existing) = self.row_index.get(&tile) {
                if let Some(slot) = self.packed_edges.get_mut(existing as usize) {
                    *slot = edge;
                }
                existing
            } else if let Some(reused) = self.free_rows.pop() {
                if let Some(slot) = self.packed_edges.get_mut(reused as usize) {
                    *slot = edge;
                }
                self.row_index.insert(tile, reused);
                reused
            } else {
                let next = self.packed_edges.len() as u32;
                self.packed_edges.push(edge);
                self.row_index.insert(tile, next);
                next
            };
            self.dirty_rows.push(row);
        }
    }

    /// Rebuilds the edges of the tiles the change reaches: those added or given another DEM,
    /// and those touching a tile that was added, removed or changed, whose shared vertices
    /// may now belong to another mesh. The edges of every other tile are kept in place.
    fn update(&mut self, tiles: &Tiles, max_rows: usize) {
        let sigs = std::mem::take(&mut self.sig_scratch);
        let mut after =
            FastTileMap::with_capacity_and_hasher(sigs.len(), BuildHasherDefault::default());
        for (tile, source) in &sigs {
            after.insert(*tile, source);
        }
        let changed: Vec<WorldTileCoords> = {
            let mut before = FastTileMap::with_capacity_and_hasher(
                self.signature.len(),
                BuildHasherDefault::default(),
            );
            for (tile, source) in &self.signature {
                before.insert(*tile, source);
            }
            after
                .iter()
                .filter(|(tile, source)| before.get(*tile) != Some(*source))
                .map(|(tile, _)| *tile)
                .chain(
                    before
                        .keys()
                        .filter(|tile| !after.contains_key(*tile))
                        .copied(),
                )
                .collect()
        };
        let mut selection_tiles =
            FastTileMap::with_capacity_and_hasher(sigs.len(), BuildHasherDefault::default());
        for (tile, source) in &sigs {
            selection_tiles.insert(*tile, source.as_ref().map(|v| v.0));
        }
        let selection = Sources::new(selection_tiles);
        let affected: Vec<WorldTileCoords> = selection
            .tiles
            .keys()
            .copied()
            .filter(|tile| changed.iter().any(|other| touches(*tile, *other)))
            .collect();
        let dems = Dems::new(&selection, tiles);
        self.evict_stale_rows(&after);
        self.write_affected_rows(&affected, &selection, &dems);
        if self.packed_edges.len() > max_rows && after.len() <= max_rows {
            self.compact_rows();
        }
        drop(after);
        self.rebuilt = affected.len();
        self.sig_scratch = sigs;
        std::mem::swap(&mut self.signature, &mut self.sig_scratch);
        self.generation = self.generation.wrapping_add(1).max(1);
    }
}

/// Whether two tiles share any point, an edge or a corner, across the antimeridian too; a tile
/// touches itself.
fn touches(a: WorldTileCoords, b: WorldTileCoords) -> bool {
    let bounds = |tile: WorldTileCoords| {
        let size = 1.0 / sample::scale_of(u8::from(tile.z));
        [f64::from(tile.x) * size, f64::from(tile.y) * size, size]
    };
    let ([ax, ay, asize], [bx, by, bsize]) = (bounds(a), bounds(b));
    let overlaps = |a0: f64, a1: f64, b0: f64, b1: f64| a0 <= b1 + 1e-12 && b0 <= a1 + 1e-12;
    overlaps(ay, ay + asize, by, by + bsize)
        && [-1.0, 0.0, 1.0]
            .into_iter()
            .any(|shift| overlaps(ax, ax + asize, bx + shift, bx + shift + bsize))
}

#[cfg(test)]
mod tests;
