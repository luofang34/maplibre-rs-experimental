//! The triangles the terrain is drawn with, rebuilt on the CPU where a line of sight needs them.
//!
//! Each rendered tile is a grid of [`TERRAIN_MESH_SIZE`] cells a side, every cell split into two
//! triangles along its diagonal from the north-west to the south-east corner, with vertices at
//! the elevation the tile samples there. The tiles of the first and last row close the globe
//! with a fan from their outer edge to the pole at sea level. Lines are intersected with these
//! flat triangles in unit-sphere space, the chords the GPU draws, not with the curved surface.

use cgmath::{InnerSpace, Point2, Vector3};

use super::{DrawnTerrain, MAX_MERCATOR_Y};
use crate::{
    coords::{LatLon, WorldTileCoords},
    projection::globe::lat_lon_to_unit_sphere,
    terrain::mesh::TERRAIN_MESH_SIZE,
};

const CELLS: f64 = TERRAIN_MESH_SIZE as f64;
const PARALLEL_EPSILON: f64 = 1e-18;

/// A cell of a rendered tile's grid, or of its polar fan when `row` is `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Cell {
    pub tile: WorldTileCoords,
    pub column: u32,
    pub row: Option<u32>,
}

impl Cell {
    /// The cell of `tile` holding `mercator`, clamped into the tile.
    pub fn of(tile: WorldTileCoords, mercator: Point2<f64>) -> Self {
        let (u, v) = local(tile, mercator);
        Self {
            tile,
            column: u.floor().clamp(0.0, CELLS - 1.0) as u32,
            row: Some(v.floor().clamp(0.0, CELLS - 1.0) as u32),
        }
    }
}

/// Grid coordinates of `mercator` in `tile`, `0..=128` across the tile.
fn local(tile: WorldTileCoords, mercator: Point2<f64>) -> (f64, f64) {
    let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
    let west = f64::from(tile.x) / scale;
    let x = mercator.x - (mercator.x - west + 0.5).floor();
    (
        (x * scale - f64::from(tile.x)) * CELLS,
        (mercator.y * scale - f64::from(tile.y)) * CELLS,
    )
}

impl DrawnTerrain<'_> {
    /// Grid vertex `(column, row)` of `tile` in unit-sphere space and its elevation, `None`
    /// where the tile's DEM has not loaded.
    fn vertex(&self, tile: WorldTileCoords, column: u32, row: u32) -> Option<(Vector3<f64>, f64)> {
        let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
        let mercator = Point2::new(
            (f64::from(tile.x) + f64::from(column) / CELLS) / scale,
            ((f64::from(tile.y) + f64::from(row) / CELLS) / scale).min(MAX_MERCATOR_Y),
        );
        let sample = self
            .index
            .sample_in(self.tiles, tile, mercator.x, mercator.y);
        sample.dem_loaded.then(|| {
            let location = mercator_to_lat_lon(mercator);
            let point =
                lat_lon_to_unit_sphere(location) * self.body.unit_radius_at(sample.elevation);
            (point, sample.elevation)
        })
    }

    /// The drawn triangles of `cell`, empty where its DEM has not loaded.
    pub(super) fn triangles(&self, cell: Cell) -> Vec<[Vector3<f64>; 3]> {
        let Cell { tile, column, row } = cell;
        let Some(row) = row else {
            // The fan from the tile's outer edge to the pole.
            let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
            let north = tile.y == 0;
            let (edge, pole) = if north {
                (0, Vector3::unit_y())
            } else if f64::from(tile.y + 1) >= scale {
                (TERRAIN_MESH_SIZE, -Vector3::unit_y())
            } else {
                return Vec::new();
            };
            return match (
                self.vertex(tile, column, edge),
                self.vertex(tile, column + 1, edge),
            ) {
                (Some((a, _)), Some((b, _))) => vec![[a, b, pole]],
                _ => Vec::new(),
            };
        };
        let corner = |dx: u32, dy: u32| self.vertex(tile, column + dx, row + dy).map(|v| v.0);
        match (corner(0, 0), corner(0, 1), corner(1, 1), corner(1, 0)) {
            (Some(nw), Some(sw), Some(se), Some(ne)) => vec![[nw, sw, se], [nw, se, ne]],
            _ => Vec::new(),
        }
    }
}

/// Where the line `eye + direction * t` crosses the triangle, either face.
pub(super) fn intersect(
    eye: Vector3<f64>,
    direction: Vector3<f64>,
    [a, b, c]: [Vector3<f64>; 3],
) -> Option<f64> {
    let (ab, ac) = (b - a, c - a);
    let p = direction.cross(ac);
    let determinant = ab.dot(p);
    if determinant.abs() < PARALLEL_EPSILON {
        return None;
    }
    let inverse = 1.0 / determinant;
    let s = eye - a;
    let u = s.dot(p) * inverse;
    let q = s.cross(ab);
    let v = direction.dot(q) * inverse;
    // A hair of slack keeps a line through a shared edge from slipping between two triangles.
    let slack = 1e-9;
    if u < -slack || v < -slack || u + v > 1.0 + slack {
        return None;
    }
    Some(ac.dot(q) * inverse)
}

fn mercator_to_lat_lon(mercator: Point2<f64>) -> LatLon {
    LatLon::new(
        super::mercator_y_to_latitude(mercator.y).to_degrees(),
        mercator.x * 360.0 - 180.0,
    )
}
