use super::*;

#[inline]
pub(super) fn scale_of(z: u8) -> f64 {
    if z < 31 {
        f64::from(1_u32 << z)
    } else {
        2_f64.powi(i32::from(z))
    }
}

pub(super) struct Sources {
    pub(super) tiles: FastTileMap<Option<WorldTileCoords>>,
    zooms: Vec<u8>,
}

impl Sources {
    pub(super) fn new(tiles: FastTileMap<Option<WorldTileCoords>>) -> Self {
        let mut zooms: Vec<_> = tiles.keys().map(|tile| u8::from(tile.z)).collect();
        zooms.sort_unstable();
        zooms.dedup();
        Self { tiles, zooms }
    }
}

/// The loaded DEM of every source a selection samples, looked up once per rebuild so the
/// per-vertex sampling does no tile query.
pub(super) struct Dems<'a>(FastTileMap<&'a DemTile>);

impl<'a> Dems<'a> {
    pub(super) fn new(sources: &Sources, tiles: &'a Tiles) -> Self {
        let mut map = FastTileMap::with_capacity_and_hasher(
            sources.tiles.len(),
            BuildHasherDefault::default(),
        );
        for source in sources.tiles.values().flatten() {
            if !map.contains_key(source) {
                if let Some(DemTileComponent::Loaded(dem)) =
                    tiles.query::<&DemTileComponent>(*source)
                {
                    map.insert(*source, &**dem);
                }
            }
        }
        Self(map)
    }
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

pub(super) fn build_edges(tile: WorldTileCoords, sources: &Sources, dems: &Dems) -> EdgeHeights {
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

struct SideOwnerGrid<'a> {
    owner_tile: WorldTileCoords,
    owner_scale: f64,
    owner_x: f64,
    owner_y: f64,
    owner_dem: Option<ResolvedDem<'a>>,
    sources: &'a Sources,
    dems: &'a Dems<'a>,
}

impl<'a> SideOwnerGrid<'a> {
    fn new(owner_tile: WorldTileCoords, sources: &'a Sources, dems: &'a Dems<'a>) -> Self {
        Self {
            owner_tile,
            owner_scale: scale_of(u8::from(owner_tile.z)),
            owner_x: f64::from(owner_tile.x),
            owner_y: f64::from(owner_tile.y),
            owner_dem: ResolvedDem::lookup(owner_tile, sources, dems),
            sources,
            dems,
        }
    }

    fn eval_sample(&self, p: [f64; 2], interior: bool) -> f64 {
        if interior {
            return self.owner_dem.map_or(0.0, |d| d.sample(p));
        }
        let ep = owner(p, self.owner_tile, self.sources);
        sample_source(p, self.sources.tiles.get(&ep).copied().flatten(), self.dems)
    }

    fn eval_horizontal_col(&self, gx: f64, (gy0, gy1): (f64, f64), single_row: bool) -> (f64, f64) {
        let px = (self.owner_x + gx / N as f64) / self.owner_scale;
        let interior = single_row && gx > 0.0 && gx < N as f64;
        let top = self.eval_sample(
            [px, (self.owner_y + gy0 / N as f64) / self.owner_scale],
            interior,
        );
        let bot = if single_row {
            top
        } else {
            self.eval_sample(
                [px, (self.owner_y + gy1 / N as f64) / self.owner_scale],
                false,
            )
        };
        (top, bot)
    }

    fn eval_vertical_row(&self, gy: f64, (gx0, gx1): (f64, f64), single_col: bool) -> (f64, f64) {
        let py = (self.owner_y + gy / N as f64) / self.owner_scale;
        let interior = single_col && gy > 0.0 && gy < N as f64;
        let left = self.eval_sample(
            [(self.owner_x + gx0 / N as f64) / self.owner_scale, py],
            interior,
        );
        let right = if single_col {
            left
        } else {
            self.eval_sample(
                [(self.owner_x + gx1 / N as f64) / self.owner_scale, py],
                false,
            )
        };
        (left, right)
    }
}

fn fill_horizontal_side(
    result: &mut EdgeHeights,
    side: usize,
    (tx, py, tile_scale): (f64, f64, f64),
    side_owner: WorldTileCoords,
    sources: &Sources,
    dems: &Dems,
) {
    let grid = SideOwnerGrid::new(side_owner, sources, dems);
    let y = (py * grid.owner_scale - grid.owner_y).clamp(0.0, 1.0) * N as f64;
    let gy_pair = (y.floor(), y.ceil());
    let fy = y.fract();
    let single_row = fy == 0.0;
    let mut cached = (-1_i32, 0.0_f64, 0.0_f64);

    for (i, slot) in result.samples.iter_mut().enumerate().skip(1) {
        let px = (tx + i as f64 / N as f64) / tile_scale;
        let x = (px * grid.owner_scale - grid.owner_x)
            .rem_euclid(grid.owner_scale)
            .min(1.0)
            * N as f64;
        let gx0 = x.floor();
        let fx = x.fract();
        let gx0_i = gx0 as i32;
        let (t0, b0) = if gx0_i == cached.0 {
            (cached.1, cached.2)
        } else {
            grid.eval_horizontal_col(gx0, gy_pair, single_row)
        };
        let val = if fx == 0.0 {
            cached = (gx0_i, t0, b0);
            if single_row {
                t0
            } else {
                t0 * (1.0 - fy) + b0 * fy
            }
        } else {
            let gx1 = x.ceil();
            let (t1, b1) = grid.eval_horizontal_col(gx1, gy_pair, single_row);
            cached = (gx1 as i32, t1, b1);
            let top = t0 * (1.0 - fx) + t1 * fx;
            if single_row {
                top
            } else {
                top * (1.0 - fy) + (b0 * (1.0 - fx) + b1 * fx) * fy
            }
        };
        if let Some(cell) = slot.get_mut(side) {
            *cell = val as f32;
        }
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
    let grid = SideOwnerGrid::new(side_owner, sources, dems);
    let x = (px * grid.owner_scale - grid.owner_x)
        .rem_euclid(grid.owner_scale)
        .min(1.0)
        * N as f64;
    let gx_pair = (x.floor(), x.ceil());
    let fx = x.fract();
    let single_col = fx == 0.0;
    let mut cached = (-1_i32, 0.0_f64, 0.0_f64);

    for (i, slot) in result.samples.iter_mut().enumerate().skip(1) {
        let py = (ty + i as f64 / N as f64) / tile_scale;
        let y = (py * grid.owner_scale - grid.owner_y).clamp(0.0, 1.0) * N as f64;
        let gy0 = y.floor();
        let fy = y.fract();
        let gy0_i = gy0 as i32;
        let (l0, r0) = if gy0_i == cached.0 {
            (cached.1, cached.2)
        } else {
            grid.eval_vertical_row(gy0, gx_pair, single_col)
        };
        let val = if fy == 0.0 {
            cached = (gy0_i, l0, r0);
            if single_col {
                l0
            } else {
                l0 * (1.0 - fx) + r0 * fx
            }
        } else {
            let gy1 = y.ceil();
            let (l1, r1) = grid.eval_vertical_row(gy1, gx_pair, single_col);
            cached = (gy1 as i32, l1, r1);
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
        if let Some(cell) = slot.get_mut(side) {
            *cell = val as f32;
        }
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

pub(super) fn shared_height(
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
