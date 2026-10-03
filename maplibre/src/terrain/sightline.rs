//! Lines of sight across the drawn globe terrain: whether a point in the air or on the ground
//! can be seen from the camera, and where a screen ray first meets the ground.
//!
//! A line is followed from the eye through the rendered tiles of the coverage index, with the
//! DEM, exaggeration and ancestor fallback the terrain is drawn with, and meets the ground on
//! the triangles the GPU draws. Where the line runs above a tile's highest ground it skips
//! ahead to where it could first reach it; near the ground it advances half a mesh cell at a
//! time and tests every triangle it passes over, so a ridge is not stepped over however long
//! and flat the line runs. A rendered tile no DEM describes yet is unknown,
//! neither sea level nor clear: a line that passes below the body's highest ground there
//! cannot be called unobstructed. Ground the frame does not draw at all, outside the rendered
//! tiles, hides nothing, as in the picture.

use std::collections::HashMap;

use cgmath::{InnerSpace, Point2, Vector3};

use crate::{
    coords::{LatLon, WorldTileCoords},
    projection::{
        body::Body,
        globe::{lat_lon_to_unit_sphere, unit_sphere_to_lat_lon},
    },
    tcs::tiles::Tiles,
    terrain::coverage::TerrainCoverageIndex,
};

mod mesh;
mod pick;
mod target;

use mesh::Cell;

pub use pick::{pick_globe_terrain, TerrainPick};
pub use target::{target_view, TargetAltitude, TargetView, Visibility};

/// Latitude beyond which the globe is closed by the polar caps at sea level.
const MAX_VALID_LATITUDE: f64 = 85.051_128_779_806_59;
const MAX_MERCATOR_Y: f64 = 1.0 - 1e-9;
/// Steps per mesh cell near the ground.
const STEPS_PER_CELL: f64 = 2.0;
const CELLS: f64 = crate::terrain::mesh::TERRAIN_MESH_SIZE as f64;
/// Smallest step in metres, so a line converging on a bound still advances.
const MIN_STEP_METERS: f64 = 0.01;

/// The drawn terrain a line of sight is followed across.
#[derive(Clone, Copy)]
pub struct DrawnTerrain<'a> {
    /// The rendered tiles and the DEM tiles behind them.
    pub index: &'a TerrainCoverageIndex,
    /// Tiles holding the loaded DEM data.
    pub tiles: &'a Tiles,
    /// The body the terrain lies on.
    pub body: Body,
}

/// Where a line first meets the drawn ground.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GroundCrossing {
    /// Distance along the line, in unit-sphere lengths.
    pub t: f64,
    /// The ground point.
    pub location: LatLon,
    /// Drawn elevation of the ground there in metres, exaggeration included.
    pub elevation: f64,
    /// Whether the ground is a polar cap rather than a tile.
    pub polar: bool,
}

/// What a line of sight passes before its end.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Crossing {
    /// The first drawn ground it meets.
    pub ground: Option<GroundCrossing>,
    /// Where it first runs below the body's highest ground over a rendered tile no DEM
    /// describes, if that is before `ground`.
    pub unknown: Option<f64>,
}

/// The ground under one point of a line.
struct Probe {
    mercator: Point2<f64>,
    /// Height of the point above the mean radius in metres.
    height: f64,
    /// Distance of the point from the body's center, in radii.
    radius: f64,
    polar: bool,
    /// The rendered tile under the point, and whether its DEM has loaded.
    tile: Option<(WorldTileCoords, bool)>,
}

impl DrawnTerrain<'_> {
    /// Highest ground anywhere, drawn or not yet described, in metres.
    pub(crate) fn ceiling(&self) -> f64 {
        self.index
            .max_elevation()
            .max(self.body.highest_ground_meters * self.index.exaggeration().max(1.0))
    }

    /// Height of the drawn ground at a location: where the line up from the body's center
    /// meets the triangles drawn there. `None` outside the rendered tiles and where the DEM
    /// has not loaded.
    pub(crate) fn ground_at(&self, location: LatLon) -> Option<f64> {
        let up = lat_lon_to_unit_sphere(location);
        let mercator = lat_lon_to_mercator(location);
        let cells = self.cells_at(mercator, location.latitude.abs() > MAX_VALID_LATITUDE);
        cells
            .into_iter()
            .flat_map(|cell| self.triangles(cell))
            .filter_map(|triangle| mesh::intersect(Vector3::new(0.0, 0.0, 0.0), up, triangle))
            .reduce(f64::max)
            .map(|radius| (radius - 1.0) * self.body.radius_meters)
    }

    /// The drawn cells over `mercator`: the rendered tile's grid cell, or under a polar cap
    /// the fan of the last tile in its column.
    fn cells_at(&self, mercator: Point2<f64>, polar: bool) -> Vec<Cell> {
        if polar {
            let edge = if mercator.y < 0.5 {
                0.0
            } else {
                MAX_MERCATOR_Y
            };
            return self
                .index
                .rendered_tile_at(mercator.x, edge)
                .map(|tile| Cell {
                    row: None,
                    ..Cell::of(tile, mercator)
                })
                .into_iter()
                .collect();
        }
        self.index
            .rendered_tile_at(mercator.x, mercator.y)
            .map(|tile| Cell::of(tile, mercator))
            .into_iter()
            .collect()
    }

    fn probe(&self, point: Vector3<f64>) -> Probe {
        let radius = point.magnitude();
        let location = unit_sphere_to_lat_lon(point / radius);
        let mercator = lat_lon_to_mercator(location);
        let polar = location.latitude.abs() > MAX_VALID_LATITUDE;
        let tile = (!polar)
            .then(|| self.index.rendered_tile_at(mercator.x, mercator.y))
            .flatten()
            .map(|tile| {
                let sample = self
                    .index
                    .sample_in(self.tiles, tile, mercator.x, mercator.y);
                (tile, sample.dem_loaded)
            });
        Probe {
            mercator,
            height: (radius - 1.0) * self.body.radius_meters,
            radius,
            polar,
            tile,
        }
    }

    /// Follows the line `eye + direction * t` for `t` in `start..end`, unit-sphere lengths,
    /// and reports the first drawn ground it meets and any unknown ground it passes before.
    /// Ground for which `skip` holds is passed through, as a target's own ground is.
    pub(crate) fn follow(
        &self,
        eye: Vector3<f64>,
        direction: Vector3<f64>,
        (start, end): (f64, f64),
        skip: impl Fn(LatLon) -> bool,
    ) -> Crossing {
        let mut crossing = Crossing::default();
        // The stretch from the previous point, unless it was proven clear of ground.
        let mut previous: Option<(f64, Probe)> = None;
        // Neighbouring stretches pass over the same cells.
        let mut drawn: HashMap<Cell, Vec<[Vector3<f64>; 3]>> = HashMap::new();
        let mut t = start;
        loop {
            let probe = self.probe(eye + direction * t);
            if let Some((from, last)) = &previous {
                let ends = [last, &probe];
                let hit = self.first_triangle(eye, direction, (*from, t), ends, &skip, &mut drawn);
                if hit.is_some() {
                    crossing.ground = hit;
                    return crossing;
                }
            }
            let unknown = matches!(probe.tile, Some((_, false))) && probe.height <= self.ceiling();
            if unknown && crossing.unknown.is_none() {
                crossing.unknown = Some(t);
            }
            if t >= end {
                return crossing;
            }
            let (step, clear) = self.safe_step(&probe);
            let next = (t + step).min(end);
            previous = (!clear).then_some((t, probe));
            t = next;
        }
    }

    /// The first drawn triangle the stretch `from..to` of the line crosses, among the cells
    /// under its two ends. A stretch near the ground is shorter than half a cell, so the
    /// cells under its ends and the two between them diagonally hold all it passes over.
    fn first_triangle(
        &self,
        eye: Vector3<f64>,
        direction: Vector3<f64>,
        (from, to): (f64, f64),
        ends: [&Probe; 2],
        skip: &impl Fn(LatLon) -> bool,
        drawn: &mut HashMap<Cell, Vec<[Vector3<f64>; 3]>>,
    ) -> Option<GroundCrossing> {
        let mut cells: Vec<Cell> = Vec::new();
        let mut seen: std::collections::HashSet<Cell> = std::collections::HashSet::new();
        let mut add = |cell: Cell| {
            if seen.insert(cell) {
                cells.push(cell);
            }
        };
        for end in ends {
            for cell in self.cells_at(end.mercator, end.polar) {
                for other in ends {
                    let mut corner = Cell::of(cell.tile, other.mercator);
                    if cell.row.is_none() {
                        corner.row = None;
                    }
                    let crossed = [
                        cell,
                        corner,
                        Cell {
                            column: corner.column,
                            ..cell
                        },
                        Cell {
                            column: cell.column,
                            row: corner.row,
                            ..cell
                        },
                    ];
                    for crossed in crossed {
                        add(crossed);
                    }
                }
            }
        }
        // Near a pole a short stretch sweeps across many sectors of the cap's fan; every sector
        // between the longitudes of its ends is tested.
        if let Some(polar) = ends.iter().find(|end| end.polar) {
            let edge = if polar.mercator.y < 0.5 {
                0.0
            } else {
                MAX_MERCATOR_Y
            };
            let sector =
                1.0 / (2_f64.powi(i32::from(self.index.finest_zoom().unwrap_or(0))) * CELLS);
            let from_x = ends[0].mercator.x;
            let arc = (ends[1].mercator.x - from_x + 0.5).rem_euclid(1.0) - 0.5;
            let steps = (arc.abs() / (sector * 0.5)).ceil() as usize;
            for step in 0..=steps {
                let x = from_x + arc * step as f64 / steps.max(1) as f64;
                for cell in self.cells_at(Point2::new(x - x.floor(), edge), true) {
                    add(cell);
                }
            }
        }
        let slack = 1e-12;
        cells
            .into_iter()
            .flat_map(|cell| {
                let polar = cell.row.is_none();
                drawn
                    .entry(cell)
                    .or_insert_with(|| self.triangles(cell))
                    .clone()
                    .into_iter()
                    .map(move |triangle| (triangle, polar))
            })
            .filter_map(|(triangle, polar)| {
                let t = mesh::intersect(eye, direction, triangle)?;
                (t >= from - slack && t <= to + slack).then_some((t, polar))
            })
            .filter_map(|(t, polar)| {
                let point = eye + direction * t;
                let location = unit_sphere_to_lat_lon(point.normalize());
                (!skip(location)).then(|| GroundCrossing {
                    t,
                    location,
                    elevation: (point.magnitude() - 1.0) * self.body.radius_meters,
                    polar,
                })
            })
            .min_by(|a, b| a.t.total_cmp(&b.t))
    }

    /// How far along a line, in unit-sphere lengths, the point of `probe` can move without
    /// passing drawn ground it could meet.
    ///
    /// A straight line's height above a sphere falls by at most the distance travelled, so a
    /// point that far above the highest ground of its tile cannot reach that ground before
    /// it has moved as far, nor leave the tile before it has moved the distance to its edge,
    /// scaled down when the point lies below the mean radius and its ground track runs faster.
    /// Near the ground it moves half a mesh cell, so the cells under the ends of each stretch
    /// hold every triangle the stretch passes over. Returns the step and whether the stretch
    /// it covers is proven clear of ground.
    fn safe_step(&self, probe: &Probe) -> (f64, bool) {
        let radius = self.body.radius_meters;
        let minimum = MIN_STEP_METERS / radius;
        let bounded = |bound: f64, fine: f64| (bound.max(fine).max(minimum), bound >= fine);
        if probe.polar {
            return bounded(
                (probe.height - self.ceiling()) / radius,
                self.finest_step(probe.mercator),
            );
        }
        let Some((tile, _)) = probe.tile else {
            // Outside the rendered tiles nothing is drawn until the line enters one, and every
            // rendered tile's edges lie on the grid of the finest zoom.
            let zoom = self.index.finest_zoom().unwrap_or(0);
            let scale = 2_f64.powi(i32::from(zoom));
            let cell = WorldTileCoords {
                x: ((probe.mercator.x - probe.mercator.x.floor()) * scale).floor() as i32,
                y: (probe.mercator.y * scale).floor() as i32,
                z: crate::coords::ZoomLevel::new(zoom),
            };
            let inside = track_budget(
                TileFootprint::of(cell).distance_to_edge(probe.mercator),
                probe.radius,
            );
            let (step, _) = bounded(inside, self.finest_step(probe.mercator));
            // Nothing is drawn here, but a step past the cell may reach a rendered tile.
            return (step, inside >= step);
        };
        let footprint = TileFootprint::of(tile);
        let edge = footprint.distance_to_edge(probe.mercator);
        // Within a cell of the edge the neighbour may be several zooms finer, its cells a
        // fraction of this tile's; a stretch into it advances half of the finest cell, so the
        // cells under its ends still hold everything it passes over.
        let fine = if edge < footprint.width / CELLS {
            self.finest_step(probe.mercator)
        } else {
            footprint.width / (CELLS * STEPS_PER_CELL)
        };
        let highest = self
            .index
            .tile_elevation_range(tile)
            .map_or(self.ceiling(), |range| range.max_meters);
        let above = (probe.height - highest) / radius;
        bounded(above.min(track_budget(edge, probe.radius)), fine)
    }

    /// The finest step any rendered tile asks for.
    /// Half the width of the finest rendered cell at `mercator`, measured on the pole-ward
    /// row of the finest zoom's tile there, the narrowest any rendered cell can be; over a cap,
    /// the last row's.
    fn finest_step(&self, mercator: Point2<f64>) -> f64 {
        let zoom = self.index.finest_zoom().unwrap_or(0);
        let scale = 2_f64.powi(i32::from(zoom));
        let tile = WorldTileCoords {
            x: ((mercator.x - mercator.x.floor()) * scale).floor() as i32,
            y: (mercator.y.clamp(0.0, MAX_MERCATOR_Y) * scale).floor() as i32,
            z: crate::coords::ZoomLevel::new(zoom),
        };
        TileFootprint::of(tile).width / (CELLS * STEPS_PER_CELL)
    }
}

/// How far a point `radius` radii from the body's center can move along a straight line
/// while its ground track moves at most `angle` radians: the track turns at most the distance
/// over the radius, faster than the distance itself below the mean radius.
fn track_budget(angle: f64, radius: f64) -> f64 {
    angle * radius.min(1.0)
}

/// A tile's extent in Mercator units and the shortest ground widths it spans, in radians.
struct TileFootprint {
    west: f64,
    east: f64,
    north: f64,
    south: f64,
    /// Radians per Mercator unit where the tile is narrowest: its pole-ward row.
    radians_per_unit: f64,
    /// Ground width of the tile in radians along its narrowest row.
    width: f64,
}

impl TileFootprint {
    fn of(tile: WorldTileCoords) -> Self {
        let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
        let (north, south) = (f64::from(tile.y) / scale, f64::from(tile.y + 1) / scale);
        let poleward = if north + south < 1.0 { north } else { south };
        let radians_per_unit = std::f64::consts::TAU * mercator_y_to_latitude(poleward).cos();
        Self {
            west: f64::from(tile.x) / scale,
            east: f64::from(tile.x + 1) / scale,
            north,
            south,
            radians_per_unit,
            width: radians_per_unit / scale,
        }
    }

    /// Ground distance in radians from `mercator` to the nearest edge of the tile, at least.
    fn distance_to_edge(&self, mercator: Point2<f64>) -> f64 {
        let x = mercator.x - mercator.x.floor();
        let across = (x - self.west).min(self.east - x);
        let along = (mercator.y - self.north).min(self.south - mercator.y);
        across.min(along).max(0.0) * self.radians_per_unit
    }
}

/// Mercator coordinates in `0..1` of a geographic location.
pub(crate) fn lat_lon_to_mercator(location: LatLon) -> Point2<f64> {
    let x = location.longitude / 360.0 + 0.5;
    let y = (1.0 - location.latitude.to_radians().tan().asinh() / std::f64::consts::PI) * 0.5;
    Point2::new(x, y.clamp(0.0, MAX_MERCATOR_Y))
}

/// The location of Mercator coordinates in `0..1`.
pub(crate) fn mercator_to_lat_lon(mercator: Point2<f64>) -> LatLon {
    LatLon::new(
        mercator_y_to_latitude(mercator.y).to_degrees(),
        mercator.x * 360.0 - 180.0,
    )
}

fn mercator_y_to_latitude(y: f64) -> f64 {
    (std::f64::consts::PI * (1.0 - 2.0 * y)).sinh().atan()
}

#[cfg(test)]
pub(crate) mod synthetic;
#[cfg(test)]
mod tests;
