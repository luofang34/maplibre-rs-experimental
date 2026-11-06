//! Elevation queries share the boundary surface used by the GPU.
use super::*;
use crate::terrain::{dem::DemTile, queue_system::edges::EdgeHeights};
#[cfg(feature = "headless")]
pub(crate) struct StitchedPatchGrid {
    pub(crate) dem_coords: WorldTileCoords,
    pub(crate) dem_tile: Arc<DemTile>,
    pub(crate) heights_meters: Arc<[f32]>,
    pub(crate) min_meters: f32,
    pub(crate) max_meters: f32,
    pub(crate) exaggeration: f32,
    pub(crate) stitched_to_neighbors: bool,
}

impl TerrainCoverageIndex {
    pub(crate) fn set_surface_edges(
        &mut self,
        sources: &[(
            Option<WorldTileCoords>,
            WorldTileCoords,
            Option<WorldTileCoords>,
        )],
        edges: Arc<HashMap<WorldTileCoords, EdgeHeights>>,
    ) {
        self.rendered = sources.iter().map(|(dem, tile, _)| (*tile, *dem)).collect();
        self.zooms = sources
            .iter()
            .map(|(_, tile, _)| u8::from(tile.z))
            .collect();
        self.zooms.sort_unstable_by(|a, b| b.cmp(a));
        self.zooms.dedup();
        self.edges = edges;
    }
    pub(super) fn stitched_height(
        &self,
        tile: WorldTileCoords,
        source: WorldTileCoords,
        point: [f64; 2],
        dem: &DemTile,
    ) -> Option<f64> {
        let edges = self.edges.get(&tile)?;
        let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
        let p = [
            (point[0] * scale - f64::from(tile.x)) * EXTENT,
            (point[1] * scale - f64::from(tile.y)) * EXTENT,
        ];
        if p.iter().all(|v| *v > 64.0 && *v < EXTENT - 64.0) {
            return None;
        }
        let dem_scale = 2_f64.powi(i32::from(u8::from(source.z)));
        let raw = |p: [f64; 2]| {
            dem.elevation_at_tile_coords(
                ((f64::from(tile.x) + p[0] / EXTENT) / scale * dem_scale - f64::from(source.x))
                    * EXTENT,
                ((f64::from(tile.y) + p[1] / EXTENT) / scale * dem_scale - f64::from(source.y))
                    * EXTENT,
            )
        };
        let x = p[0] / 32.0;
        let y = p[1] / 32.0;
        let a = edges.height([x.floor() * 32.0, y.floor() * 32.0], &raw);
        let b = edges.height([x.ceil() * 32.0, y.floor() * 32.0], &raw);
        let c = edges.height([x.floor() * 32.0, y.ceil() * 32.0], &raw);
        let d = edges.height([x.ceil() * 32.0, y.ceil() * 32.0], &raw);
        let (fx, fy) = (x.fract(), y.fract());
        Some(if fx > fy {
            a * (1.0 - fx) + b * (fx - fy) + d * fy
        } else {
            a * (1.0 - fy) + c * (fy - fx) + d * fx
        })
    }

    /// Evaluates the `(TERRAIN_MESH_SIZE + 1) × (TERRAIN_MESH_SIZE + 1)` (`129 × 129`) vertex
    /// height grid in metres for `tile`, applying GPU-identical boundary crack stitching when
    /// `tile` has cached neighbor [`EdgeHeights`].
    #[cfg(feature = "headless")]
    pub(crate) fn stitched_patch_grid(
        &self,
        tiles: &Tiles,
        tile: WorldTileCoords,
        fallback_dem: Option<(u8, u8, f32)>,
    ) -> Option<StitchedPatchGrid> {
        let (minzoom, maxzoom, exaggeration) = if self.exaggeration > 0.0 {
            (self.minzoom, self.maxzoom, self.exaggeration)
        } else {
            let (minz, maxz, exag) = fallback_dem?;
            (minz, maxz, f64::from(exag))
        };
        let source = self
            .rendered
            .get(&tile)
            .copied()
            .flatten()
            .or_else(|| self.loaded_dem_for(tile))
            .or_else(|| {
                let mut cur = dem_tile_coords(tile, minzoom, maxzoom)?;
                loop {
                    if matches!(
                        tiles.query::<&DemTileComponent>(cur),
                        Some(DemTileComponent::Loaded(_))
                    ) {
                        return Some(cur);
                    }
                    cur = cur.get_parent()?;
                }
            })?;
        let Some(DemTileComponent::Loaded(loaded)) = tiles.query::<&DemTileComponent>(source)
        else {
            return None;
        };
        let dem = Arc::clone(&loaded.tile);
        let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
        let dem_scale = 2_f64.powi(i32::from(u8::from(source.z)));
        let raw = |p: [f64; 2]| {
            dem.elevation_at_tile_coords(
                ((f64::from(tile.x) + p[0] / EXTENT) / scale * dem_scale - f64::from(source.x))
                    * EXTENT,
                ((f64::from(tile.y) + p[1] / EXTENT) / scale * dem_scale - f64::from(source.y))
                    * EXTENT,
            )
        };
        let side = crate::terrain::mesh::TERRAIN_MESH_SIZE as usize + 1;
        let step = EXTENT / f64::from(crate::terrain::mesh::TERRAIN_MESH_SIZE);
        let edges = self.edges.get(&tile);
        let mut heights = Vec::with_capacity(side * side);
        let mut min_m = f32::INFINITY;
        let mut max_m = f32::NEG_INFINITY;
        for row in 0..side {
            let y = row as f64 * step;
            let border_y = row < 2 || row + 2 >= side;
            for col in 0..side {
                let x = col as f64 * step;
                let unscaled = match edges {
                    Some(edge_heights) if border_y || col < 2 || col + 2 >= side => {
                        edge_heights.height([x, y], &raw)
                    }
                    _ => raw([x, y]),
                };
                let value = (unscaled * exaggeration) as f32;
                min_m = min_m.min(value);
                max_m = max_m.max(value);
                heights.push(value);
            }
        }
        Some(StitchedPatchGrid {
            dem_coords: source,
            dem_tile: dem,
            heights_meters: Arc::from(heights),
            min_meters: min_m,
            max_meters: max_m,
            exaggeration: exaggeration as f32,
            stitched_to_neighbors: edges.is_some(),
        })
    }

    /// Evaluates `(bilinear_msl_m, cell_max_msl_m)` in unexaggerated metres at normalized
    /// `[0, 1]` tile coordinates `(u, v)` on `tile`'s `128 × 128` terrain grid without
    /// allocating or stitching the remaining `16,637` vertices.
    #[cfg(feature = "headless")]
    pub(crate) fn sample_stitched_patch_cell_msl(
        &self,
        tiles: &Tiles,
        tile: WorldTileCoords,
        (u, v): (f64, f64),
        fallback_dem: Option<(u8, u8, f32)>,
    ) -> Option<(f64, f64)> {
        let (minzoom, maxzoom, exaggeration) = if self.exaggeration > 0.0 {
            (self.minzoom, self.maxzoom, self.exaggeration)
        } else {
            let (minz, maxz, exag) = fallback_dem?;
            (minz, maxz, f64::from(exag))
        };
        let source = self
            .rendered
            .get(&tile)
            .copied()
            .flatten()
            .or_else(|| self.loaded_dem_for(tile))
            .or_else(|| {
                let mut cur = dem_tile_coords(tile, minzoom, maxzoom)?;
                loop {
                    if matches!(
                        tiles.query::<&DemTileComponent>(cur),
                        Some(DemTileComponent::Loaded(_))
                    ) {
                        return Some(cur);
                    }
                    cur = cur.get_parent()?;
                }
            })?;
        let Some(DemTileComponent::Loaded(loaded)) = tiles.query::<&DemTileComponent>(source)
        else {
            return None;
        };
        let dem = &loaded.tile;
        let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
        let dem_scale = 2_f64.powi(i32::from(u8::from(source.z)));
        let raw = |p: [f64; 2]| {
            dem.elevation_at_tile_coords(
                ((f64::from(tile.x) + p[0] / EXTENT) / scale * dem_scale - f64::from(source.x))
                    * EXTENT,
                ((f64::from(tile.y) + p[1] / EXTENT) / scale * dem_scale - f64::from(source.y))
                    * EXTENT,
            )
        };
        let max_idx = crate::terrain::mesh::TERRAIN_MESH_SIZE;
        let side = max_idx as usize + 1;
        let step = EXTENT / f64::from(max_idx);
        let edges = self.edges.get(&tile);
        let vertex_exag_f32 = |col: u32, row: u32| -> f32 {
            let c = col as usize;
            let r = row as usize;
            let x = c as f64 * step;
            let y = r as f64 * step;
            let unscaled = match edges {
                Some(edge_heights) if r < 2 || r + 2 >= side || c < 2 || c + 2 >= side => {
                    edge_heights.height([x, y], &raw)
                }
                _ => raw([x, y]),
            };
            (unscaled * exaggeration) as f32
        };

        let gx = (u.clamp(0.0, 1.0) * f64::from(max_idx)) as f32;
        let gy = (v.clamp(0.0, 1.0) * f64::from(max_idx)) as f32;
        let x0 = (gx.floor().max(0.0) as u32).min(max_idx);
        let y0 = (gy.floor().max(0.0) as u32).min(max_idx);
        let x1 = x0.saturating_add(1).min(max_idx);
        let y1 = y0.saturating_add(1).min(max_idx);
        let tx = (gx - (x0 as f32)).clamp(0.0, 1.0);
        let ty = (gy - (y0 as f32)).clamp(0.0, 1.0);

        let h00 = vertex_exag_f32(x0, y0);
        let h10 = if x1 == x0 { h00 } else { vertex_exag_f32(x1, y0) };
        let h01 = if y1 == y0 { h00 } else { vertex_exag_f32(x0, y1) };
        let h11 = if x1 == x0 {
            h01
        } else if y1 == y0 {
            h10
        } else {
            vertex_exag_f32(x1, y1)
        };
        let exag = f64::from((exaggeration as f32).abs().max(1e-6));
        let top = h00 + (h10 - h00) * tx;
        let bot = h01 + (h11 - h01) * tx;
        let bil_msl = f64::from(top + (bot - top) * ty) / exag;
        let cell_max_msl = f64::from(h00.max(h10).max(h01).max(h11)) / exag;
        Some((bil_msl, cell_max_msl))
    }
}
impl EdgeHeights {
    pub(crate) fn height(&self, p: [f64; 2], raw: &impl Fn([f64; 2]) -> f64) -> f64 {
        let edge = |t: f64, side: usize| {
            let i = (t / 32.0).round().clamp(0.0, 128.0) as usize;
            f64::from(if i == 128 {
                self.last[side]
            } else {
                self.samples[i][side]
            })
        };
        let w = [p[1], EXTENT - p[1], p[0], EXTENT - p[0]].map(|d| {
            let t = (d / 64.0).clamp(0.0, 1.0);
            1.0 - t * t * (3.0 - 2.0 * t)
        });
        let delta = [
            edge(p[0], 0) - raw([p[0], 0.0]),
            edge(p[0], 1) - raw([p[0], EXTENT]),
            edge(p[1], 2) - raw([0.0, p[1]]),
            edge(p[1], 3) - raw([EXTENT, p[1]]),
        ];
        let corners = [
            edge(0.0, 0) - raw([0.0, 0.0]),
            edge(EXTENT, 0) - raw([EXTENT, 0.0]),
            edge(0.0, 1) - raw([0.0, EXTENT]),
            edge(EXTENT, 1) - raw([EXTENT, EXTENT]),
        ];
        let weights = [w[0] * w[2], w[0] * w[3], w[1] * w[2], w[1] * w[3]];
        raw(p) + w.iter().zip(delta).map(|(a, b)| a * b).sum::<f64>()
            - weights.iter().zip(corners).map(|(a, b)| a * b).sum::<f64>()
    }
}

#[cfg(test)]
mod tests;
