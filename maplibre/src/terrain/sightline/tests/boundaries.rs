//! The marcher meets the first drawn triangle on a line wherever tiles of different zooms
//! meet, with the triangles the GPU draws, and its step bounds hold where they are tightest.

use cgmath::{InnerSpace, Point2, Vector3};

use super::super::{
    mesh::{intersect, Cell},
    track_budget, DrawnTerrain, TileFootprint,
};
use super::*;
use crate::{
    coords::ZoomLevel,
    tcs::tiles::Tiles,
    terrain::{
        coverage::TerrainCoverageIndex,
        mesh::{create_terrain_mesh, TERRAIN_MESH_SIZE},
        DemTileComponent, LoadedDem,
    },
};

/// Zoom-11 tiles with the zoom-14 tiles of one of them, east of the scene, drawing a 300 m
/// ridge a few fine cells inside the fine tiles at `shift` fine cells from their west edge.
fn mixed_zooms(shift: f64) -> (Tiles, TerrainCoverageIndex, Vec<WorldTileCoords>, f64) {
    let ridge = lat_lon_to_mercator(offset(SCENE, 12_000.0, 0.0));
    let (tx, ty) = (
        (ridge.x * 2048.0).floor() as i32,
        (ridge.y * 2048.0).floor() as i32,
    );
    let fine_cell = 1.0 / 16384.0 / 128.0;
    let crest = f64::from(tx) / 2048.0 + (2.5 + shift) * fine_cell;
    let height = move |q: Point2<f64>| {
        if (q.x - crest).abs() < 1.2 * fine_cell {
            300.0
        } else {
            0.0
        }
    };
    let fine: Vec<WorldTileCoords> = (0..64)
        .map(|i| WorldTileCoords {
            x: tx * 8 + i % 8,
            y: ty * 8 + i / 8,
            z: ZoomLevel::new(14),
        })
        .collect();
    let rendered: Vec<WorldTileCoords> = block(SCENE, ZOOM, 4)
        .into_iter()
        .filter(|tile| (tile.x, tile.y) != (tx, ty))
        .chain(fine.iter().copied())
        .collect();
    let mut loaded = block(SCENE, DEM_ZOOM, 3);
    loaded.extend((0..16).map(|i| WorldTileCoords {
        x: tx * 4 + i % 4,
        y: ty * 4 + i / 4,
        z: ZoomLevel::new(13),
    }));
    let mut tiles = Tiles::default();
    for coords in &loaded {
        tiles
            .spawn_mut(*coords)
            .expect("valid coords")
            .insert(DemTileComponent::Loaded(LoadedDem::new(dem_tile(
                *coords, &height,
            ))));
    }
    let index = TerrainCoverageIndex::build(rendered, &tiles, &source(14));
    (tiles, index, fine, crest)
}

/// The first triangle the line meets among every cell near the zoom boundary, along the
/// rows the scene's latitude crosses.
fn brute_force(
    terrain: DrawnTerrain<'_>,
    fine: &[WorldTileCoords],
    eye: Vector3<f64>,
    direction: Vector3<f64>,
) -> Option<f64> {
    let latitude = lat_lon_to_mercator(SCENE).y;
    let rows = |zoom: u8| {
        let row = latitude * 2_f64.powi(i32::from(zoom));
        let tile = row.floor() as i32;
        let cell = ((row - row.floor()) * 128.0).floor() as u32;
        (
            tile,
            cell.saturating_sub(4)..(cell + 5).min(TERRAIN_MESH_SIZE),
        )
    };
    let (fine_row, fine_cells) = rows(14);
    let (coarse_row, coarse_cells) = rows(11);
    let west = fine.iter().map(|tile| tile.x).min().expect("fine tiles");
    let fine_tile = WorldTileCoords {
        x: west,
        y: fine_row,
        z: ZoomLevel::new(14),
    };
    let coarse_tile = WorldTileCoords {
        x: west / 8 - 1,
        y: coarse_row,
        z: ZoomLevel::new(11),
    };
    let cells = (0..12)
        .flat_map(|column| fine_cells.clone().map(move |row| (fine_tile, column, row)))
        .chain((110..128).flat_map(|column| {
            coarse_cells
                .clone()
                .map(move |row| (coarse_tile, column, row))
        }));
    cells
        .flat_map(|(tile, column, row)| {
            terrain.triangles(Cell {
                tile,
                column,
                row: Some(row),
            })
        })
        .filter_map(|triangle| intersect(eye, direction, triangle))
        .filter(|t| *t > 0.0)
        .reduce(f64::min)
}

#[test]
fn a_line_into_tiles_three_zooms_finer_meets_their_first_triangle() {
    for shift in [2.0, 3.3] {
        let (tiles, index, fine, crest) = mixed_zooms(shift);
        let terrain = DrawnTerrain {
            index: &index,
            tiles: &tiles,
            body: Body::EARTH,
        };
        let mut checked = 0;
        for step in 0..40 {
            let camera = camera(offset(SCENE, -7.0 * f64::from(step), 0.0), 85.0, 90.0, 0.0);
            let eye = camera.camera_position();
            for below in [5.0, 40.0, 100.0] {
                let target = LatLon::new(SCENE.latitude, crest * 360.0 - 180.0);
                let aim = target_view(
                    &camera,
                    None,
                    target,
                    TargetAltitude::Drawn {
                        meters: 300.0 - below,
                    },
                );
                let pixel = aim.pixel.expect("in front of the camera");
                let direction = camera.ray_direction_from_pixel(pixel).expect("ray");
                let Some(first) = brute_force(terrain, &fine, eye, direction) else {
                    continue;
                };
                checked += 1;
                let TerrainPick::Ground(hit) = pick_globe_terrain(&camera, terrain, pixel) else {
                    panic!("shift {shift}, step {step}, {below} m under the crest: no ground");
                };
                let point =
                    crate::projection::globe::lat_lon_to_unit_sphere(location(hit.mercator))
                        * Body::EARTH.unit_radius_at(hit.elevation);
                let along = (point - eye).magnitude();
                assert!(
                    (along - first).abs() * Body::EARTH.radius_meters < 1.0,
                    "shift {shift}, step {step}, {below} m under the crest: the pick lies {} m \
                     past the first drawn triangle",
                    (along - first) * Body::EARTH.radius_meters
                );
            }
        }
        assert!(checked > 60, "shift {shift}: {checked} lines checked");
    }
}

#[test]
fn the_triangles_are_the_gpu_mesh_s() {
    let ground = Ground::around(SCENE, hills);
    let terrain = ground.terrain();
    let tile = block(SCENE, ZOOM, 0)[0];
    let mesh = create_terrain_mesh(TERRAIN_MESH_SIZE);
    let row = TERRAIN_MESH_SIZE + 1;
    for (column, cell_row) in [(0, 0), (3, 5), (127, 64)] {
        let first = (cell_row * row + column) as usize;
        // The mesh lists each cell's two triangles together, six indices a cell.
        let at = mesh
            .indices
            .chunks(6)
            .position(|chunk| chunk[0] as usize == first)
            .expect("the cell's triangles");
        let grid = |index: u32| (index % row, index / row);
        let corner = |(x, y): (u32, u32)| {
            let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
            let mercator = Point2::new(
                (f64::from(tile.x) + f64::from(x) / 128.0) / scale,
                (f64::from(tile.y) + f64::from(y) / 128.0) / scale,
            );
            let elevation = ground
                .index
                .sample_in(&ground.tiles, tile, mercator.x, mercator.y)
                .elevation;
            crate::projection::globe::lat_lon_to_unit_sphere(location(mercator))
                * Body::EARTH.unit_radius_at(elevation)
        };
        let gpu: Vec<[Vector3<f64>; 3]> = mesh.indices[at * 6..at * 6 + 6]
            .chunks(3)
            .map(|triangle| [0, 1, 2].map(|i| corner(grid(triangle[i]))))
            .collect();
        let cpu = terrain.triangles(Cell {
            tile,
            column,
            row: Some(cell_row),
        });
        assert_eq!(cpu.len(), 2);
        for (cpu, gpu) in cpu.iter().zip(&gpu) {
            for (a, b) in cpu.iter().zip(gpu) {
                assert!(
                    (a - b).magnitude() < 1e-15,
                    "cell ({column},{cell_row}) differs"
                );
            }
        }
    }
}

#[test]
fn the_step_bounds_hold_where_they_are_tightest() {
    // A tile at 70 degrees: its pole-ward row is the narrowest, so the distance to an edge
    // measured there is never longer than the true one.
    let tile = WorldTileCoords {
        x: 600,
        y: 250,
        z: ZoomLevel::new(10),
    };
    let footprint = TileFootprint::of(tile);
    let scale = 1024.0;
    for (u, v) in [
        (0.5, 0.02),
        (0.5, 0.98),
        (0.02, 0.5),
        (0.97, 0.03),
        (0.5, 0.5),
    ] {
        let mercator = Point2::new((600.0 + u) / scale, (250.0 + v) / scale);
        let here = crate::projection::globe::lat_lon_to_unit_sphere(location(mercator));
        let edges = [
            Point2::new(600.0 / scale, mercator.y),
            Point2::new(601.0 / scale, mercator.y),
            Point2::new(mercator.x, 250.0 / scale),
            Point2::new(mercator.x, 251.0 / scale),
        ];
        let nearest = edges
            .iter()
            .map(|edge| {
                here.angle(crate::projection::globe::lat_lon_to_unit_sphere(location(
                    *edge,
                )))
                .0
            })
            .fold(f64::INFINITY, f64::min);
        assert!(
            footprint.distance_to_edge(mercator) <= nearest * (1.0 + 1e-9),
            "({u},{v}): bound {} past the edge {nearest}",
            footprint.distance_to_edge(mercator)
        );
    }
    // Below the mean radius the ground track turns faster than the point moves.
    for radius in [0.9999, 1.0, 1.001] {
        let angle = 1e-4;
        let budget = track_budget(angle, radius);
        let start = Vector3::new(0.0, 0.0, radius);
        let moved = start + Vector3::new(1.0, 0.0, 0.0) * budget;
        assert!(
            start.angle(moved).0 <= angle * (1.0 + 1e-9),
            "radius {radius}"
        );
    }
}

/// Spikes one DEM sample wide on a grid of every third sample, the shape most likely to fall in
/// a cell a line only crosses at a corner.
fn spikes(mercator: Point2<f64>) -> f64 {
    let sample = |v: f64| (v * 2_f64.powi(i32::from(DEM_ZOOM)) * 256.0).floor() as i64;
    if sample(mercator.x) % 3 == 0 && sample(mercator.y) % 3 == 0 {
        400.0
    } else {
        0.0
    }
}

#[test]
fn a_line_crossing_cells_at_their_corners_meets_the_first_triangle() {
    let ground = Ground::around(SCENE, spikes);
    let terrain = ground.terrain();
    let camera = camera(SCENE, 85.0, 45.0, 0.0);
    let eye = camera.camera_position();
    let cell = TileFootprint::of(block(SCENE, ZOOM, 0)[0]).width / 128.0;
    let mut checked = 0;
    for y in (700..1100).step_by(23) {
        for x in (300..2000).step_by(37) {
            let pixel = Point2::new(f64::from(x) + 0.5, f64::from(y) + 0.5);
            let TerrainPick::Ground(hit) = pick_globe_terrain(&camera, terrain, pixel) else {
                continue;
            };
            let direction = camera.ray_direction_from_pixel(pixel).expect("ray");
            let point = crate::projection::globe::lat_lon_to_unit_sphere(location(hit.mercator))
                * Body::EARTH.unit_radius_at(hit.elevation);
            let found = (point - eye).magnitude();
            // Every cell around the track over the last three cells before the pick.
            let mut cells: Vec<Cell> = Vec::new();
            let mut t = found - 3.0 * cell;
            while t <= found {
                let under = lat_lon_to_mercator(crate::projection::globe::unit_sphere_to_lat_lon(
                    (eye + direction * t).normalize(),
                ));
                if let Some(tile) = ground.index.rendered_tile_at(under.x, under.y) {
                    let centre = Cell::of(tile, under);
                    let row = centre.row.expect("grid cell");
                    for (dx, dy) in (0..9).map(|i| (i % 3, i / 3)) {
                        let around = Cell {
                            tile,
                            column: (centre.column + dx).saturating_sub(1).min(127),
                            row: Some((row + dy).saturating_sub(1).min(127)),
                        };
                        if !cells.contains(&around) {
                            cells.push(around);
                        }
                    }
                }
                t += cell / 8.0;
            }
            let first = cells
                .into_iter()
                .flat_map(|cell| terrain.triangles(cell))
                .filter_map(|triangle| intersect(eye, direction, triangle))
                .filter(|t| *t > 0.0)
                .fold(found, f64::min);
            assert!(
                (found - first) * Body::EARTH.radius_meters < 1.0,
                "{pixel:?}: the pick lies {} m past the first drawn triangle",
                (found - first) * Body::EARTH.radius_meters
            );
            checked += 1;
        }
    }
    assert!(checked > 200, "{checked} lines checked");
}

/// A zoom-6 camera at 88.5 degrees with its radius given, as a camera crossing the pole has it:
/// the derived radius grows towards the pole.
fn near_the_pole(pitch: f64, bearing: f64) -> crate::projection::globe::camera::GlobeCameraState {
    crate::projection::globe::camera::GlobeCameraState::new(
        crate::projection::globe::camera::GlobeCameraOptions {
            width: 2330.0,
            height: 1800.0,
            field_of_view_degrees: 36.869_897_645_844_02,
            center: LatLon::new(88.5, 10.0),
            world_size: 512.0 * 2_f64.powi(6),
            bearing_degrees: bearing,
            pitch_degrees: pitch,
            roll_degrees: 0.0,
            center_offset: Point2::new(0.0, 0.0),
            body: Body::EARTH,
            target_elevation_meters: 0.0,
            radius_pixels: Some(crate::projection::globe::scale::radius_pixels(6.0, 85.0)),
        },
    )
    .expect("camera")
}

#[test]
fn a_line_grazing_a_pole_meets_the_first_sector_of_its_cap() {
    // The top row of zoom-3 tiles all round the north pole, level at 500 m, so the cap is a
    // fan of 1024 narrow sectors meeting at the pole.
    let rendered: Vec<WorldTileCoords> = (0..8)
        .map(|x| WorldTileCoords {
            x,
            y: 0,
            z: ZoomLevel::new(3),
        })
        .collect();
    let loaded: Vec<WorldTileCoords> = (0..4)
        .map(|x| WorldTileCoords {
            x,
            y: 0,
            z: ZoomLevel::new(2),
        })
        .collect();
    let ground = Ground::new(&rendered, &loaded, |_| 500.0);
    let terrain = ground.terrain();
    let sectors: Vec<[Vector3<f64>; 3]> = rendered
        .iter()
        .flat_map(|tile| {
            (0..TERRAIN_MESH_SIZE).map(move |column| Cell {
                tile: *tile,
                column,
                row: None,
            })
        })
        .flat_map(|cell| terrain.triangles(cell))
        .collect();
    let mut checked = 0;
    for (pitch, bearing) in [(50.0, 0.0), (70.0, 30.0), (20.0, 200.0)] {
        let camera = near_the_pole(pitch, bearing);
        let eye = camera.camera_position();
        for from_pole in [0.001, 0.01, 0.1] {
            for step in 0..24 {
                let aim = LatLon::new(90.0 - from_pole, f64::from(step) * 15.0 - 180.0);
                let view =
                    target_view(&camera, None, aim, TargetAltitude::Drawn { meters: -200.0 });
                // Aimed under the sea-level sphere, at the cap's chord; only the pixel matters.
                let Some(pixel) = view
                    .pixel
                    .filter(|_| view.visibility != Visibility::OutsideFrustum)
                else {
                    continue;
                };
                let direction = camera.ray_direction_from_pixel(pixel).expect("ray");
                let Some(first) = sectors
                    .iter()
                    .filter_map(|triangle| intersect(eye, direction, *triangle))
                    .filter(|t| *t > 0.0)
                    .reduce(f64::min)
                else {
                    continue;
                };
                let TerrainPick::PolarCap(hit) = pick_globe_terrain(&camera, terrain, pixel) else {
                    panic!("{aim:?}: the line meets the cap, the pick does not");
                };
                let point = crate::projection::globe::lat_lon_to_unit_sphere(hit.location)
                    * Body::EARTH.unit_radius_at(hit.elevation);
                let found = (point - eye).magnitude();
                assert!(
                    (found - first).abs() * Body::EARTH.radius_meters < 1.0,
                    "{aim:?}, pitch {pitch}: the pick lies {} m from the first sector",
                    (found - first) * Body::EARTH.radius_meters
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 60, "{checked} lines checked");
}
