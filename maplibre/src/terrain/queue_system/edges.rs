//! Shared boundary elevations for independently loaded terrain meshes.
use std::{
    collections::HashMap,
    hash::{BuildHasherDefault, Hasher},
    sync::Arc,
};

use sample::{build_edges, Dems, Sources};
#[cfg(test)]
use sample::{scale_of, shared_height};

use crate::{
    coords::{WorldTileCoords, EXTENT},
    tcs::tiles::Tiles,
    terrain::{
        dem::DemTile, mesh::TERRAIN_MESH_SIZE, resources::TerrainTileUniforms, DemRevision,
        DemTileComponent,
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

#[derive(Clone, Copy, Debug, bytemuck_derive::Pod, bytemuck_derive::Zeroable)]
#[repr(C)]
pub(crate) struct EdgeHeights {
    pub(crate) samples: [[f32; 4]; N],
    pub(crate) last: [f32; 4],
}

impl Default for EdgeHeights {
    fn default() -> Self {
        Self {
            samples: [[0.0; 4]; N],
            last: [0.0; 4],
        }
    }
}

type Signature = Vec<(WorldTileCoords, Option<(WorldTileCoords, DemRevision)>)>;

#[derive(Default)]
pub(crate) struct EdgeCache {
    signature: Signature,
    sig_scratch: Signature,
    pub(crate) samples: Arc<HashMap<WorldTileCoords, EdgeHeights>>,
    row_index: FastTileMap<u32>,
    packed_edges: Vec<EdgeHeights>,
    generation: u64,
    /// How many tiles' edges the last change rebuilt.
    pub(crate) rebuilt: usize,
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
            self.update(tiles);
        }
    }

    pub(super) fn packed_edges(&self) -> &[EdgeHeights] {
        &self.packed_edges
    }

    /// Assigns each tile's `edge_index[0]` into its uniform block and returns `(generation, fallback)`.
    /// When `packed_edges` fits within `max_rows` and matches an unchanged signature, callers skip
    /// copying and uploading `EdgeHeights` to the GPU.
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
        self.sync_signature(sources, tiles);
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
            edges.push(self.samples.get(tile).copied().unwrap_or_default());
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
        self.sync_signature(sources, tiles);
        let mut edges = Vec::with_capacity(sources.len());
        for (index, ((_, tile, _), uniform)) in sources.iter().zip(uniforms.iter_mut()).enumerate()
        {
            let edge = self.samples.get(tile).copied().unwrap_or_default();
            uniform.edge_index[0] = index as u32;
            edges.push(edge);
        }
        edges
    }

    /// Rebuilds the edges of the tiles the change reaches: those added or given another DEM,
    /// and those touching a tile that was added, removed or changed, whose shared vertices
    /// may now belong to another mesh. The edges of every other tile are kept.
    fn update(&mut self, tiles: &Tiles) {
        let mut before = FastTileMap::with_capacity_and_hasher(
            self.signature.len(),
            BuildHasherDefault::default(),
        );
        for (tile, source) in &self.signature {
            before.insert(*tile, source);
        }
        let mut after = FastTileMap::with_capacity_and_hasher(
            self.sig_scratch.len(),
            BuildHasherDefault::default(),
        );
        for (tile, source) in &self.sig_scratch {
            after.insert(*tile, source);
        }
        let changed: Vec<WorldTileCoords> = after
            .iter()
            .filter(|(tile, source)| before.get(*tile) != Some(*source))
            .map(|(tile, _)| *tile)
            .chain(
                before
                    .keys()
                    .filter(|tile| !after.contains_key(*tile))
                    .copied(),
            )
            .collect();
        let mut selection_tiles = FastTileMap::with_capacity_and_hasher(
            self.sig_scratch.len(),
            BuildHasherDefault::default(),
        );
        for (tile, source) in &self.sig_scratch {
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
        let mut samples = HashMap::with_capacity(after.len());
        for (tile, edges) in self.samples.iter() {
            if after.contains_key(tile) {
                samples.insert(*tile, *edges);
            }
        }
        for tile in &affected {
            samples.insert(*tile, build_edges(*tile, &selection, &dems));
        }
        self.rebuilt = affected.len();
        self.row_index.clear();
        self.packed_edges.clear();
        self.packed_edges.reserve(self.sig_scratch.len());
        for (tile, _) in &self.sig_scratch {
            if self.row_index.contains_key(tile) {
                continue;
            }
            let row = self.packed_edges.len() as u32;
            self.row_index.insert(*tile, row);
            self.packed_edges
                .push(samples.get(tile).copied().unwrap_or_default());
        }
        self.samples = Arc::new(samples);
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
