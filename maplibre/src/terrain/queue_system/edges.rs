//! Shared boundary elevations for independently loaded terrain meshes.
use crate::{
    coords::{WorldTileCoords, EXTENT},
    tcs::tiles::Tiles,
    terrain::{mesh::TERRAIN_MESH_SIZE, resources::TerrainTileUniforms, DemTileComponent},
};
use std::{collections::HashMap, sync::Arc};

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

#[derive(Default)]
pub(super) struct EdgeCache {
    signature: Vec<(WorldTileCoords, Option<(WorldTileCoords, u32)>)>,
    pub(super) samples: Arc<HashMap<WorldTileCoords, EdgeHeights>>,
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
                        Some(DemTileComponent::Loaded(dem)) => Some((source, dem.revision)),
                        _ => None,
                    });
                (*tile, revision)
            })
            .collect();
        signature.sort_by_key(|(tile, _)| (u8::from(tile.z), tile.y, tile.x));
        if signature != self.signature {
            let selection = Sources::new(
                signature
                    .iter()
                    .map(|(tile, source)| (*tile, source.map(|v| v.0)))
                    .collect(),
            );
            self.samples = Arc::new(
                selection
                    .tiles
                    .keys()
                    .map(|tile| (*tile, build_edges(*tile, &selection, tiles)))
                    .collect(),
            );
            self.signature = signature;
        }
        for ((_, tile, _), uniform) in sources.iter().zip(uniforms) {
            if let Some(edges) = self.samples.get(tile) {
                uniform.edge_heights = *edges;
            }
        }
    }
}

fn build_edges(tile: WorldTileCoords, sources: &Sources, tiles: &Tiles) -> EdgeHeights {
    let mut result = EdgeHeights::default();
    let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
    for i in 0..=N {
        let t = i as f64 / N as f64;
        let heights = [[t, 0.0], [t, 1.0], [0.0, t], [1.0, t]].map(|uv| {
            let point = [
                (f64::from(tile.x) + uv[0]) / scale,
                (f64::from(tile.y) + uv[1]) / scale,
            ];
            shared_height(point, tile, sources, tiles) as f32
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
    tiles: &Tiles,
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
            tiles,
        )
    };
    let top =
        sample(x.floor(), y.floor()) * (1.0 - x.fract()) + sample(x.ceil(), y.floor()) * x.fract();
    let bottom =
        sample(x.floor(), y.ceil()) * (1.0 - x.fract()) + sample(x.ceil(), y.ceil()) * x.fract();
    top * (1.0 - y.fract()) + bottom * y.fract()
}

fn sample_source(point: [f64; 2], source: Option<WorldTileCoords>, tiles: &Tiles) -> f64 {
    let Some(source) = source else {
        return 0.0;
    };
    let Some(DemTileComponent::Loaded(dem)) = tiles.query::<&DemTileComponent>(source) else {
        return 0.0;
    };
    let scale = 2_f64.powi(i32::from(u8::from(source.z)));
    let x = (point[0] * scale - f64::from(source.x))
        .rem_euclid(scale)
        .min(1.0);
    let y = (point[1] * scale - f64::from(source.y)).clamp(0.0, 1.0);
    dem.tile.elevation_at_tile_coords(x * EXTENT, y * EXTENT)
}

#[cfg(test)]
mod tests;
