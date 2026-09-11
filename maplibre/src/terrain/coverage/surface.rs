//! Elevation queries share the boundary surface used by the GPU.
use super::*;
use crate::terrain::{dem::DemTile, queue_system::edges::EdgeHeights};
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
}
impl EdgeHeights {
    fn height(&self, p: [f64; 2], raw: &impl Fn([f64; 2]) -> f64) -> f64 {
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
