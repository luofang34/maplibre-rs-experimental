//! Shared boundary elevations for independently loaded terrain meshes.
use std::{
    collections::HashMap,
    hash::{BuildHasherDefault, Hasher},
    sync::Arc,
};

use crate::{
    coords::{WorldTileCoords, EXTENT},
    tcs::tiles::Tiles,
    terrain::{
        dem::DemTile, mesh::TERRAIN_MESH_SIZE, resources::TerrainTileUniforms, DemRevision,
        DemTileComponent,
    },
};

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

#[inline]
fn scale_of(z: u8) -> f64 {
    if z < 31 {
        f64::from(1_u32 << z)
    } else {
        2_f64.powi(i32::from(z))
    }
}

struct Sources {
    tiles: FastTileMap<Option<WorldTileCoords>>,
    zooms: Vec<u8>,
}

impl Sources {
    fn new(tiles: FastTileMap<Option<WorldTileCoords>>) -> Self {
        let mut zooms: Vec<_> = tiles.keys().map(|tile| u8::from(tile.z)).collect();
        zooms.sort_unstable();
        zooms.dedup();
        Self { tiles, zooms }
    }
}

/// The loaded DEM of every source a selection samples, looked up once per rebuild so the
/// per-vertex sampling does no tile query.
struct Dems<'a>(FastTileMap<&'a DemTile>);

impl<'a> Dems<'a> {
    fn new(sources: &Sources, tiles: &'a Tiles) -> Self {
        let mut map = FastTileMap::with_capacity_and_hasher(
            sources.tiles.len(),
            BuildHasherDefault::default(),
        );
        for source in sources.tiles.values().flatten() {
            if !map.contains_key(source) {
                if let Some(DemTileComponent::Loaded(dem)) =
                    tiles.query::<&DemTileComponent>(*source)
                {
                    map.insert(*source, dem.tile.as_ref());
                }
            }
        }
        Self(map)
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
pub(crate) struct EdgeCache {
    signature: Signature,
    pub(crate) samples: Arc<HashMap<WorldTileCoords, EdgeHeights>>,
    /// How many tiles' edges the last change rebuilt.
    pub(crate) rebuilt: usize,
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
    ) -> Vec<EdgeHeights> {
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
    fn update(&mut self, signature: Signature, tiles: &Tiles) {
        let mut before = FastTileMap::with_capacity_and_hasher(
            self.signature.len(),
            BuildHasherDefault::default(),
        );
        for (tile, source) in &self.signature {
            before.insert(*tile, source);
        }
        let mut after =
            FastTileMap::with_capacity_and_hasher(signature.len(), BuildHasherDefault::default());
        for (tile, source) in &signature {
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
        let mut selection_tiles =
            FastTileMap::with_capacity_and_hasher(signature.len(), BuildHasherDefault::default());
        for (tile, source) in &signature {
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
        self.samples = Arc::new(samples);
        self.signature = signature;
    }
}

/// Whether two tiles share any point, an edge or a corner, across the antimeridian too; a tile
/// touches itself.
fn touches(a: WorldTileCoords, b: WorldTileCoords) -> bool {
    let bounds = |tile: WorldTileCoords| {
        let size = 1.0 / scale_of(u8::from(tile.z));
        [f64::from(tile.x) * size, f64::from(tile.y) * size, size]
    };
    let ([ax, ay, asize], [bx, by, bsize]) = (bounds(a), bounds(b));
    let overlaps = |a0: f64, a1: f64, b0: f64, b1: f64| a0 <= b1 + 1e-12 && b0 <= a1 + 1e-12;
    overlaps(ay, ay + asize, by, by + bsize)
        && [-1.0, 0.0, 1.0]
            .into_iter()
            .any(|shift| overlaps(ax, ax + asize, bx + shift, bx + shift + bsize))
}

#[derive(Clone, Copy)]
struct ResolvedDem<'a> {
    dem: &'a DemTile,
    coords: WorldTileCoords,
    scale: f64,
}

impl<'a> ResolvedDem<'a> {
    #[inline]
    fn lookup(tile: WorldTileCoords, sources: &Sources, dems: &'a Dems<'a>) -> Option<Self> {
        let source = sources.tiles.get(&tile).copied().flatten()?;
        let dem = dems.0.get(&source).copied()?;
        Some(Self {
            dem,
            coords: source,
            scale: scale_of(u8::from(source.z)),
        })
    }

    #[inline]
    fn sample(self, point: [f64; 2]) -> f64 {
        let x = (point[0] * self.scale - f64::from(self.coords.x))
            .rem_euclid(self.scale)
            .min(1.0);
        let y = (point[1] * self.scale - f64::from(self.coords.y)).clamp(0.0, 1.0);
        self.dem.elevation_at_tile_coords(x * EXTENT, y * EXTENT)
    }
}

fn build_edges(tile: WorldTileCoords, sources: &Sources, dems: &Dems) -> EdgeHeights {
    let mut result = EdgeHeights::default();
    let scale = scale_of(u8::from(tile.z));
    let tx = f64::from(tile.x);
    let ty = f64::from(tile.y);

    // Each of the four corners is shared by two incident edges.
    let nw = shared_height([tx / scale, ty / scale], tile, sources, dems) as f32;
    let ne = shared_height([(tx + 1.0) / scale, ty / scale], tile, sources, dems) as f32;
    let sw = shared_height([tx / scale, (ty + 1.0) / scale], tile, sources, dems) as f32;
    let se = shared_height(
        [(tx + 1.0) / scale, (ty + 1.0) / scale],
        tile,
        sources,
        dems,
    ) as f32;
    result.samples[0] = [nw, sw, nw, ne];
    result.last = [ne, se, sw, se];

    let can_share_side_owner = sources
        .zooms
        .first()
        .is_none_or(|&min_z| u8::from(tile.z).saturating_sub(min_z) <= 15);
    if !can_share_side_owner {
        for (i, slot) in result.samples.iter_mut().enumerate().skip(1) {
            let t = i as f64 / N as f64;
            *slot = [[t, 0.0], [t, 1.0], [0.0, t], [1.0, t]].map(|uv| {
                let point = [(tx + uv[0]) / scale, (ty + uv[1]) / scale];
                shared_height(point, tile, sources, dems) as f32
            });
        }
        return result;
    }

    // Horizontal edges: side 0 (north, v = 0) and side 1 (south, v = 1).
    for (side, v_off) in [(0_usize, 0.0_f64), (1_usize, 1.0_f64)] {
        let py = (ty + v_off) / scale;
        let side_owner = owner([(tx + 0.5) / scale, py], tile, sources);
        fill_horizontal_side(
            &mut result,
            side,
            (tx, py, scale),
            side_owner,
            sources,
            dems,
        );
    }
    // Vertical edges: side 2 (west, u = 0) and side 3 (east, u = 1).
    for (side, u_off) in [(2_usize, 0.0_f64), (3_usize, 1.0_f64)] {
        let px = (tx + u_off) / scale;
        let side_owner = owner([px, (ty + 0.5) / scale], tile, sources);
        fill_vertical_side(
            &mut result,
            side,
            (px, ty, scale),
            side_owner,
            sources,
            dems,
        );
    }
    result
}

fn fill_horizontal_side(
    result: &mut EdgeHeights,
    side: usize,
    (tx, py, tile_scale): (f64, f64, f64),
    side_owner: WorldTileCoords,
    sources: &Sources,
    dems: &Dems,
) {
    let owner_scale = scale_of(u8::from(side_owner.z));
    let owner_x = f64::from(side_owner.x);
    let owner_y = f64::from(side_owner.y);
    let owner_dem = ResolvedDem::lookup(side_owner, sources, dems);
    let y = (py * owner_scale - owner_y).clamp(0.0, 1.0) * N as f64;
    let gy0 = y.floor();
    let gy1 = y.ceil();
    let fy = y.fract();
    let single_row = fy == 0.0;

    let mut cached_gx = -1_i32;
    let mut cached_top = 0.0_f64;
    let mut cached_bot = 0.0_f64;

    let eval_col = |gx: f64| -> (f64, f64) {
        let px = (owner_x + gx / N as f64) / owner_scale;
        let interior = single_row && gx > 0.0 && gx < N as f64;
        let top = if interior {
            owner_dem.map_or(0.0, |d| {
                d.sample([px, (owner_y + gy0 / N as f64) / owner_scale])
            })
        } else {
            let p = [px, (owner_y + gy0 / N as f64) / owner_scale];
            let ep = owner(p, side_owner, sources);
            sample_source(p, sources.tiles.get(&ep).copied().flatten(), dems)
        };
        let bot = if single_row {
            top
        } else {
            let p = [px, (owner_y + gy1 / N as f64) / owner_scale];
            let ep = owner(p, side_owner, sources);
            sample_source(p, sources.tiles.get(&ep).copied().flatten(), dems)
        };
        (top, bot)
    };

    for (i, slot) in result.samples.iter_mut().enumerate().skip(1) {
        let px = (tx + i as f64 / N as f64) / tile_scale;
        let x = (px * owner_scale - owner_x)
            .rem_euclid(owner_scale)
            .min(1.0)
            * N as f64;
        let gx0 = x.floor();
        let fx = x.fract();
        let gx0_i = gx0 as i32;
        let (t0, b0) = if gx0_i == cached_gx {
            (cached_top, cached_bot)
        } else {
            eval_col(gx0)
        };
        let val = if fx == 0.0 {
            cached_gx = gx0_i;
            cached_top = t0;
            cached_bot = b0;
            if single_row {
                t0
            } else {
                t0 * (1.0 - fy) + b0 * fy
            }
        } else {
            let gx1 = x.ceil();
            let (t1, b1) = eval_col(gx1);
            cached_gx = gx1 as i32;
            cached_top = t1;
            cached_bot = b1;
            let top = t0 * (1.0 - fx) + t1 * fx;
            if single_row {
                top
            } else {
                let bot = b0 * (1.0 - fx) + b1 * fx;
                top * (1.0 - fy) + bot * fy
            }
        };
        slot[side] = val as f32;
    }
}

fn fill_vertical_side(
    result: &mut EdgeHeights,
    side: usize,
    (px, ty, tile_scale): (f64, f64, f64),
    side_owner: WorldTileCoords,
    sources: &Sources,
    dems: &Dems,
) {
    let owner_scale = scale_of(u8::from(side_owner.z));
    let owner_x = f64::from(side_owner.x);
    let owner_y = f64::from(side_owner.y);
    let owner_dem = ResolvedDem::lookup(side_owner, sources, dems);
    let x = (px * owner_scale - owner_x)
        .rem_euclid(owner_scale)
        .min(1.0)
        * N as f64;
    let gx0 = x.floor();
    let gx1 = x.ceil();
    let fx = x.fract();
    let single_col = fx == 0.0;

    let mut cached_gy = -1_i32;
    let mut cached_left = 0.0_f64;
    let mut cached_right = 0.0_f64;

    let eval_row = |gy: f64| -> (f64, f64) {
        let py = (owner_y + gy / N as f64) / owner_scale;
        let interior = single_col && gy > 0.0 && gy < N as f64;
        let left = if interior {
            owner_dem.map_or(0.0, |d| {
                d.sample([(owner_x + gx0 / N as f64) / owner_scale, py])
            })
        } else {
            let p = [(owner_x + gx0 / N as f64) / owner_scale, py];
            let ep = owner(p, side_owner, sources);
            sample_source(p, sources.tiles.get(&ep).copied().flatten(), dems)
        };
        let right = if single_col {
            left
        } else {
            let p = [(owner_x + gx1 / N as f64) / owner_scale, py];
            let ep = owner(p, side_owner, sources);
            sample_source(p, sources.tiles.get(&ep).copied().flatten(), dems)
        };
        (left, right)
    };

    for (i, slot) in result.samples.iter_mut().enumerate().skip(1) {
        let py = (ty + i as f64 / N as f64) / tile_scale;
        let y = (py * owner_scale - owner_y).clamp(0.0, 1.0) * N as f64;
        let gy0 = y.floor();
        let fy = y.fract();
        let gy0_i = gy0 as i32;
        let (l0, r0) = if gy0_i == cached_gy {
            (cached_left, cached_right)
        } else {
            eval_row(gy0)
        };
        let val = if fy == 0.0 {
            cached_gy = gy0_i;
            cached_left = l0;
            cached_right = r0;
            if single_col {
                l0
            } else {
                l0 * (1.0 - fx) + r0 * fx
            }
        } else {
            let gy1 = y.ceil();
            let (l1, r1) = eval_row(gy1);
            cached_gy = gy1 as i32;
            cached_left = l1;
            cached_right = r1;
            let top = if single_col {
                l0
            } else {
                l0 * (1.0 - fx) + r0 * fx
            };
            let bot = if single_col {
                l1
            } else {
                l1 * (1.0 - fx) + r1 * fx
            };
            top * (1.0 - fy) + bot * fy
        };
        slot[side] = val as f32;
    }
}

fn owner(point: [f64; 2], fallback: WorldTileCoords, sources: &Sources) -> WorldTileCoords {
    // The coarsest touching mesh owns a shared vertex, including four-way corners.
    // Both sides use this ordering, independent of draw order or tile arrival order.
    let max_z = u8::from(fallback.z);
    for &z in &sources.zooms {
        if z > max_z {
            break;
        }
        let scale = scale_of(z);
        let x = point[0] * scale;
        let y = point[1] * scale;
        let y0 = (y - 1e-7).floor() as i32;
        let y1 = (y + 1e-7).floor() as i32;
        let x0 = (x - 1e-7).floor() as i32;
        let x1 = (x + 1e-7).floor() as i32;
        let wrap = 1_i32 << z;
        let probe = |xx: i32, yy: i32| -> Option<WorldTileCoords> {
            let tile = WorldTileCoords {
                x: xx.rem_euclid(wrap),
                y: yy,
                z: z.into(),
            };
            sources.tiles.contains_key(&tile).then_some(tile)
        };
        if let Some(tile) = probe(x0, y0) {
            return tile;
        }
        if x1 != x0 {
            if let Some(tile) = probe(x1, y0) {
                return tile;
            }
        }
        if y1 != y0 {
            if let Some(tile) = probe(x0, y1) {
                return tile;
            }
            if x1 != x0 {
                if let Some(tile) = probe(x1, y1) {
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
    let scale = scale_of(u8::from(tile.z));
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
    let fx = x.fract();
    let fy = y.fract();
    let top = if fx == 0.0 {
        sample(x.floor(), y.floor())
    } else {
        sample(x.floor(), y.floor()) * (1.0 - fx) + sample(x.ceil(), y.floor()) * fx
    };
    if fy == 0.0 {
        top
    } else {
        let bottom = if fx == 0.0 {
            sample(x.floor(), y.ceil())
        } else {
            sample(x.floor(), y.ceil()) * (1.0 - fx) + sample(x.ceil(), y.ceil()) * fx
        };
        top * (1.0 - fy) + bottom * fy
    }
}

fn sample_source(point: [f64; 2], source: Option<WorldTileCoords>, dems: &Dems) -> f64 {
    let Some(source) = source else {
        return 0.0;
    };
    let Some(dem) = dems.0.get(&source) else {
        return 0.0;
    };
    let scale = scale_of(u8::from(source.z));
    let x = (point[0] * scale - f64::from(source.x))
        .rem_euclid(scale)
        .min(1.0);
    let y = (point[1] * scale - f64::from(source.y)).clamp(0.0, 1.0);
    dem.elevation_at_tile_coords(x * EXTENT, y * EXTENT)
}

#[cfg(test)]
mod tests;
