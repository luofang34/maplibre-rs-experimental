//! A feature's geometry cut to the window of the tile it is drawn in.
//!
//! The renderer tessellates and, on the globe, subdivides everything a tile carries, so a
//! polygon left whole would cost each tile it touches as much as the polygon's whole area.

use geo_types::{Coord, Rect};

use super::{Geometry, Point, Ring};
use crate::coords::clip::{line_within, ring_within};

impl Geometry {
    /// The parts of the geometry within `bounds`, `[west, north, east, south]` in world units.
    pub(super) fn within(&self, bounds: [f64; 4]) -> Self {
        let [west, north, east, south] = bounds;
        let area = Rect::new(Coord { x: west, y: north }, Coord { x: east, y: south });
        let polygons = self
            .polygons
            .iter()
            .filter_map(|rings| {
                let mut cut = rings
                    .iter()
                    .map(|ring| ring_within(&coords(ring), area))
                    .map(|ring| (ring.len() >= 4).then(|| points(&ring)));
                // Without its exterior ring a polygon has nothing for its holes to cut into.
                let exterior = cut.next()??;
                Some(std::iter::once(exterior).chain(cut.flatten()).collect())
            })
            .collect();
        let lines = self
            .lines
            .iter()
            .flat_map(|line| line_within(coords(line), area))
            .map(|run| points(&run))
            .collect();
        let points = self
            .points
            .iter()
            .filter(|point| {
                (west..=east).contains(&point[0]) && (north..=south).contains(&point[1])
            })
            .copied()
            .collect();
        Self {
            points,
            lines,
            polygons,
        }
    }
}

fn coords(ring: &[Point]) -> Vec<Coord<f64>> {
    ring.iter()
        .map(|point| Coord {
            x: point[0],
            y: point[1],
        })
        .collect()
}

fn points(coords: &[Coord<f64>]) -> Ring {
    coords.iter().map(|coord| [coord.x, coord.y]).collect()
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
