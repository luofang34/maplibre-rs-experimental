//! Shared boundary elevations for independently loaded terrain meshes.
use std::{collections::HashMap, sync::Arc};

use crate::{
    coords::{WorldTileCoords, EXTENT},
    tcs::tiles::Tiles,
    terrain::{
        dem::DemTile, mesh::TERRAIN_MESH_SIZE, resources::TerrainTileUniforms, DemRevision,
        DemTileComponent,
    },
};

const N: usize = TERRAIN_MESH_SIZE as usize;
struct Sources {
    tiles: HashMap<WorldTileCoords, Option<WorldTileCoords>>,
    zooms: Vec<u8>,
}
impl Sources {
    fn new(tiles: HashMap<WorldTileCoords, Option<WorldTileCoords>>) -> Self {
        let mut zooms: Vec<_> = tiles.keys().map(|tile| u8::from(tile.z)).collect();
        zooms.sort_unstable();
        zooms.dedup();
        Self { tiles, zooms }
    }
}

/// The loaded DEM of every source a selection samples, looked up once per rebuild so the
/// per-vertex sampling does no tile query.
struct Dems<'a>(HashMap<WorldTileCoords, &'a DemTile>);
impl<'a> Dems<'a> {
    fn new(sources: &Sources, tiles: &'a Tiles) -> Self {
        Self(
            sources
                .tiles
                .values()
                .flatten()
                .filter_map(|source| match tiles.query::<&DemTileComponent>(*source) {
                    Some(DemTileComponent::Loaded(dem)) => Some((*source, &dem.tile)),
                    _ => None,
                })
                .collect(),
        )
    }
}

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
pub(super) struct EdgeCache {
    signature: Signature,
    pub(super) samples: Arc<HashMap<WorldTileCoords, EdgeHeights>>,
    /// How many tiles' edges the last change rebuilt.
    pub(super) rebuilt: usize,
}
impl EdgeCache {
    pub(super) fn apply(
        &mut self,
        sources: &[(
            Option<WorldTileCoords>,
            WorldTileCoords,
            Option<WorldTileCoords>,
        )],
        uniforms: &mut [TerrainTileUniforms],
        tiles: &Tiles,
    ) {
        let mut signature: Vec<_> = sources
            .iter()
            .map(|(source, tile, _)| {
                let revision =
                    source.and_then(|source| match tiles.query::<&DemTileComponent>(source) {
                        Some(DemTileComponent::Loaded(dem)) => Some((source, dem.revision_key())),
                        _ => None,
                    });
                (*tile, revision)
            })
            .collect();
        signature.sort_by_key(|(tile, _)| (u8::from(tile.z), tile.y, tile.x));
        if signature != self.signature {
            self.update(signature, tiles);
        }
        for ((_, tile, _), uniform) in sources.iter().zip(uniforms) {
            if let Some(edges) = self.samples.get(tile) {
                uniform.edge_heights = *edges;
            }
        }
    }
}

impl EdgeCache {
    /// Rebuilds the edges of the tiles the change reaches: those added or given another DEM,
    /// and those touching a tile that was added, removed or changed, whose shared vertices
    /// may now belong to another mesh. The edges of every other tile are kept.
    fn update(&mut self, signature: Signature, tiles: &Tiles) {
        let before: HashMap<_, _> = self
            .signature
            .iter()
            .map(|(tile, source)| (*tile, source))
            .collect();
        let after: HashMap<_, _> = signature
            .iter()
            .map(|(tile, source)| (*tile, source))
            .collect();
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
        let selection = Sources::new(
            signature
                .iter()
                .map(|(tile, source)| (*tile, source.as_ref().map(|v| v.0)))
                .collect(),
        );
        let affected: Vec<WorldTileCoords> = selection
            .tiles
            .keys()
            .copied()
            .filter(|tile| changed.iter().any(|other| touches(*tile, *other)))
            .collect();
        let dems = Dems::new(&selection, tiles);
        let mut samples: HashMap<_, _> = self
            .samples
            .iter()
            .filter(|(tile, _)| after.contains_key(tile))
            .map(|(tile, edges)| (*tile, *edges))
            .collect();
        for tile in &affected {
            samples.insert(*tile, build_edges(*tile, &selection, &dems));
        }
        self.rebuilt = affected.len();
        self.samples = Arc::new(samples);
        self.signature = signature;
    }
}

/// Whether two tiles share any point, an edge or a corner, across the antimeridian too; a tile
/// touches itself.
fn touches(a: WorldTileCoords, b: WorldTileCoords) -> bool {
    let bounds = |tile: WorldTileCoords| {
        let size = 1.0 / 2_f64.powi(i32::from(u8::from(tile.z)));
        [f64::from(tile.x) * size, f64::from(tile.y) * size, size]
    };
    let ([ax, ay, asize], [bx, by, bsize]) = (bounds(a), bounds(b));
    let overlaps = |a0: f64, a1: f64, b0: f64, b1: f64| a0 <= b1 + 1e-12 && b0 <= a1 + 1e-12;
    overlaps(ay, ay + asize, by, by + bsize)
        && [-1.0, 0.0, 1.0]
            .into_iter()
            .any(|shift| overlaps(ax, ax + asize, bx + shift, bx + shift + bsize))
}

fn build_edges(tile: WorldTileCoords, sources: &Sources, dems: &Dems) -> EdgeHeights {
    let mut result = EdgeHeights::default();
    let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
    for i in 0..=N {
        let t = i as f64 / N as f64;
        let heights = [[t, 0.0], [t, 1.0], [0.0, t], [1.0, t]].map(|uv| {
            let point = [
                (f64::from(tile.x) + uv[0]) / scale,
                (f64::from(tile.y) + uv[1]) / scale,
            ];
            shared_height(point, tile, sources, dems) as f32
        });
        if i == N {
            result.last = heights;
        } else {
            result.samples[i] = heights;
        }
    }
    result
}

fn owner(point: [f64; 2], fallback: WorldTileCoords, sources: &Sources) -> WorldTileCoords {
    // The coarsest touching mesh owns a shared vertex, including four-way corners.
    // Both sides use this ordering, independent of draw order or tile arrival order.
    for &z in sources
        .zooms
        .iter()
        .take_while(|z| **z <= u8::from(fallback.z))
    {
        let scale = 2_f64.powi(i32::from(z));
        let x = point[0] * scale;
        let y = point[1] * scale;
        for yy in [(y - 1e-7).floor() as i32, (y + 1e-7).floor() as i32] {
            for xx in [(x - 1e-7).floor() as i32, (x + 1e-7).floor() as i32] {
                let tile = WorldTileCoords {
                    x: xx.rem_euclid(1_i32 << z),
                    y: yy,
                    z: z.into(),
                };
                if sources.tiles.contains_key(&tile) {
                    return tile;
                }
            }
        }
    }
    fallback
}

fn shared_height(
    point: [f64; 2],
    fallback: WorldTileCoords,
    sources: &Sources,
    dems: &Dems,
) -> f64 {
    let tile = owner(point, fallback, sources);
    let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
    let x = (point[0] * scale - f64::from(tile.x))
        .rem_euclid(scale)
        .min(1.0)
        * N as f64;
    let y = (point[1] * scale - f64::from(tile.y)).clamp(0.0, 1.0) * N as f64;
    let sample = |gx: f64, gy: f64| {
        // Corners can touch an even coarser mesh. The common owner provides the
        // endpoint from which every intermediate fine-edge vertex interpolates.
        let p = [
            (f64::from(tile.x) + gx / N as f64) / scale,
            (f64::from(tile.y) + gy / N as f64) / scale,
        ];
        let endpoint_owner = owner(p, tile, sources);
        sample_source(
            p,
            sources.tiles.get(&endpoint_owner).copied().flatten(),
            dems,
        )
    };
    let top =
        sample(x.floor(), y.floor()) * (1.0 - x.fract()) + sample(x.ceil(), y.floor()) * x.fract();
    let bottom =
        sample(x.floor(), y.ceil()) * (1.0 - x.fract()) + sample(x.ceil(), y.ceil()) * x.fract();
    top * (1.0 - y.fract()) + bottom * y.fract()
}

fn sample_source(point: [f64; 2], source: Option<WorldTileCoords>, dems: &Dems) -> f64 {
    let Some(source) = source else {
        return 0.0;
    };
    let Some(dem) = dems.0.get(&source) else {
        return 0.0;
    };
    let scale = 2_f64.powi(i32::from(u8::from(source.z)));
    let x = (point[0] * scale - f64::from(source.x))
        .rem_euclid(scale)
        .min(1.0);
    let y = (point[1] * scale - f64::from(source.y)).clamp(0.0, 1.0);
    dem.elevation_at_tile_coords(x * EXTENT, y * EXTENT)
}

#[cfg(test)]
mod tests;
