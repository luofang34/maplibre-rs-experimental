//! The triangles the terrain is drawn with, rebuilt on the CPU where a line of sight needs them.
//!
//! Each rendered tile is a grid of [`TERRAIN_MESH_SIZE`] cells a side, every cell split into two
//! triangles along its diagonal from the north-west to the south-east corner, with vertices at
//! the elevation the tile samples there. The tiles of the first and last row close the globe
//! with a fan from their outer edge to the pole at sea level. Lines are intersected with these
//! flat triangles in unit-sphere space, the chords the GPU draws, not with the curved surface.

use cgmath::{InnerSpace, Point2, Vector3};

use super::{mercator_to_lat_lon, DrawnTerrain, MAX_MERCATOR_Y};
use crate::{
    coords::WorldTileCoords, projection::globe::lat_lon_to_unit_sphere,
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
            return self.polar_cap_triangles(tile, column);
        };
        let corner = |dx: u32, dy: u32| self.vertex(tile, column + dx, row + dy).map(|v| v.0);
        match (corner(0, 0), corner(0, 1), corner(1, 1), corner(1, 0)) {
            (Some(nw), Some(sw), Some(se), Some(ne)) => vec![[nw, sw, se], [nw, se, ne]],
            _ => Vec::new(),
        }
    }

    fn polar_cap_triangles(&self, tile: WorldTileCoords, column: u32) -> Vec<[Vector3<f64>; 3]> {
        use crate::terrain::mesh::{
            polar_cap_colatitude_radians, polar_cap_ring_t, POLAR_CAP_RINGS,
        };
        let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
        let poles = self.index.poles();
        let (north, edge, pole_meters) = if tile.y == 0 {
            (true, 0, poles.north_meters)
        } else if f64::from(tile.y) + 1.0 >= scale {
            (false, TERRAIN_MESH_SIZE, poles.south_meters)
        } else {
            return Vec::new();
        };
        let (Some((mut prev_a, elev_a)), Some((mut prev_b, elev_b))) = (
            self.vertex(tile, column, edge),
            self.vertex(tile, column + 1, edge),
        ) else {
            return Vec::new();
        };
        let exag = if self.index.exaggeration() > 0.0 {
            self.index.exaggeration()
        } else {
            1.0
        };
        let pole_elev = f64::from(pole_meters) * exag;
        let lon_a = ((f64::from(tile.x) + f64::from(column) / CELLS) / scale)
            * std::f64::consts::TAU
            + std::f64::consts::PI;
        let lon_b = ((f64::from(tile.x) + f64::from(column + 1) / CELLS) / scale)
            * std::f64::consts::TAU
            + std::f64::consts::PI;
        let mut out = Vec::with_capacity((POLAR_CAP_RINGS as usize) * 2 - 1);
        for ring in 1..=POLAR_CAP_RINGS {
            if ring == POLAR_CAP_RINGS {
                let pole_dir = if north {
                    Vector3::unit_y()
                } else {
                    -Vector3::unit_y()
                };
                let tip = pole_dir * self.body.unit_radius_at(pole_elev);
                out.push(if north {
                    [prev_a, prev_b, tip]
                } else {
                    [prev_b, prev_a, tip]
                });
                break;
            }
            let cap_t = polar_cap_ring_t(north, ring);
            let rem = 1.0 - cap_t;
            let radial_t = 1.0 - rem * rem;
            let s = (radial_t / 0.35).clamp(0.0, 1.0);
            let blend = s * s * (3.0 - 2.0 * s);
            let ea = elev_a + (pole_elev - elev_a) * blend;
            let eb = elev_b + (pole_elev - elev_b) * blend;
            let theta = polar_cap_colatitude_radians(cap_t);
            let sin_t = theta.sin();
            let y_unit = if north { theta.cos() } else { -theta.cos() };
            let curr_a = Vector3::new(lon_a.sin() * sin_t, y_unit, lon_a.cos() * sin_t)
                * self.body.unit_radius_at(ea);
            let curr_b = Vector3::new(lon_b.sin() * sin_t, y_unit, lon_b.cos() * sin_t)
                * self.body.unit_radius_at(eb);
            if north {
                out.push([prev_a, prev_b, curr_a]);
                out.push([prev_b, curr_b, curr_a]);
            } else {
                out.push([prev_b, prev_a, curr_a]);
                out.push([curr_b, prev_b, curr_a]);
            }
            prev_a = curr_a;
            prev_b = curr_b;
        }
        out
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
