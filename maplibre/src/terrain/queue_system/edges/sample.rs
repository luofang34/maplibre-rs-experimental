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
                    map.insert(*source, dem.tile.as_ref());
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
