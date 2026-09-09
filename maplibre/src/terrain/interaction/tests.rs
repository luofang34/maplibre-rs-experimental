#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{Deg, EuclideanSpace, Point2, Vector2};
use image::{Rgba, RgbaImage};

use super::{
    camera_ground_position, finish_gesture, keep_camera_above_terrain, pan_mercator_by_pixels,
    recalculate_zoom_and_center, resolve_gesture_anchor, screen_point_to_terrain,
    zoom_mercator_around, GestureAnchor,
};
use crate::{
    coords::{LatLon, WorldCoords, Zoom, TILE_SIZE},
    render::{
        projection::view_region_for_projection,
        tile_view_pattern::DEFAULT_TILE_SIZE,
        view_state::{ViewState, ViewStatePadding},
    },
    style::Style,
    tcs::world::World,
    terrain::{
        coverage::TerrainCoverageIndex,
        dem::DemTile,
        request_system::{dem_ancestor_coords, dem_tile_coords},
        source::dem_source,
        DemTileComponent, LoadedDem,
    },
    window::PhysicalSize,
};

const TERRARIUM: [f64; 4] = [256.0, 1.0, 1.0 / 256.0, 32768.0];
const INNSBRUCK: LatLon = LatLon {
    latitude: 47.27574,
    longitude: 11.39085,
};

fn terrarium_pixel(elevation: f64) -> Rgba<u8> {
    let value = elevation + 32768.0;
    let red = (value / 256.0).floor();
    let green = value - red * 256.0;
    Rgba([red as u8, green as u8, 0, 255])
}

fn flat_tile(elevation: f64) -> DemTile {
    let mut image = RgbaImage::new(4, 4);
    for pixel in image.pixels_mut() {
        *pixel = terrarium_pixel(elevation);
    }
    DemTile::from_image(&image, TERRARIUM).expect("tile decodes")
}

fn style(globe: bool) -> Style {
    let projection = if globe {
        r#","projection":{"type":"globe"}"#
    } else {
        ""
    };
    serde_json::from_str(&format!(
        r#"{{"version":8,"sources":{{"dem":{{"type":"raster-dem","tiles":["https://dem.example/{{z}}/{{x}}/{{y}}.png"],"tileSize":256,"maxzoom":12,"encoding":"terrarium"}}}},"layers":[],"terrain":{{"source":"dem","exaggeration":1}}{projection}}}"#
    ))
    .expect("style parses")
}

fn view(zoom: f64, pitch: f64) -> ViewState {
    let zoom = Zoom::new(zoom);
    let mut view = ViewState::new(
        PhysicalSize::new(1200, 800).expect("valid size"),
        WorldCoords::from_lat_lon(INNSBRUCK, zoom),
        zoom,
        Deg(0.0),
        cgmath::Rad(0.6435011087932844),
    );
    view.set_max_pitch(Deg(85.0));
    view.camera_mut().set_pitch(Deg(pitch));
    view
}

/// A world whose terrain is a flat plateau at `elevation` metres under the whole view.
fn plateau(style: &Style, view: &ViewState, elevation: f64) -> World {
    let mut world = World::default();
    let dem = dem_source(style).expect("terrain source");
    let level = view.zoom().zoom_level(DEFAULT_TILE_SIZE);
    let region = view_region_for_projection(style, view, &world, level, ViewStatePadding::Loose)
        .expect("covering succeeds")
        .expect("region exists");
    let (camera, _) = camera_ground_position(view);
    let camera_tile =
        WorldCoords::at_ground(camera.x, camera.y).into_world_tile(level, view.zoom());
    for coords in region.iter().chain(std::iter::once(camera_tile)) {
        let Some(dem_coords) = dem_tile_coords(coords, dem.minzoom, dem.maxzoom) else {
            continue;
        };
        for target in [
            Some(dem_coords),
            dem_ancestor_coords(dem_coords, dem.minzoom),
        ]
        .into_iter()
        .flatten()
        {
            if world.tiles.exists(target) {
                continue;
            }
            world
                .tiles
                .spawn_mut(target)
                .expect("valid coords")
                .insert(DemTileComponent::Loaded(LoadedDem::new(flat_tile(
                    elevation,
                ))));
        }
    }
    let rendered = view_region_for_projection(style, view, &world, level, ViewStatePadding::Tight)
        .expect("covering succeeds")
        .expect("region exists");
    let index = TerrainCoverageIndex::build(rendered.iter(), &world.tiles, &dem);
    world.resources.insert(index);
    world
}

fn assert_close(actual: f64, expected: f64, tolerance: f64, what: &str) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{what}: {actual} differs from {expected} by more than {tolerance}"
    );
}

#[test]
fn a_ray_through_the_center_hits_the_plateau_at_its_elevation() {
    let style = style(false);
    let mut view = view(12.0, 70.0);
    view.set_center_elevation(1500.0);
    let world = plateau(&style, &view, 1500.0);

    let hit = screen_point_to_terrain(&style, &view, &world, Point2::new(600.0, 400.0))
        .expect("the center looks at the plateau");

    assert_close(hit.elevation, 1500.0, 1e-6, "hit elevation");
    let world_size = TILE_SIZE * 2_f64.powf(12.0);
    let center = view.camera().position();
    assert_close(hit.mercator.x * world_size, center.x, 1e-3, "hit x");
    assert_close(hit.mercator.y * world_size, center.y, 1e-3, "hit y");
}

#[test]
fn a_sky_pixel_misses_the_terrain_and_anchors_the_gesture_on_the_center() {
    let style = style(false);
    let mut view = view(12.0, 70.0);
    view.set_center_elevation(500.0);
    let world = plateau(&style, &view, 500.0);
    let sky = Point2::new(600.0, 5.0);

    assert!(screen_point_to_terrain(&style, &view, &world, sky).is_none());
    assert_eq!(
        resolve_gesture_anchor(&style, &view, &world, sky),
        GestureAnchor {
            pixel: Point2::new(600.0, 400.0),
            elevation: None,
        }
    );
}

#[test]
fn a_surface_pixel_anchors_at_its_elevation_unless_it_is_at_the_center() {
    let style = style(false);
    let mut view = view(12.0, 60.0);
    view.set_center_elevation(500.0);
    let world = plateau(&style, &view, 500.0);

    let off_center = resolve_gesture_anchor(&style, &view, &world, Point2::new(600.0, 300.0));
    let at_center = resolve_gesture_anchor(&style, &view, &world, Point2::new(600.0, 400.0));

    assert_eq!(off_center.pixel, Point2::new(600.0, 300.0));
    assert_close(
        off_center.elevation.expect("usable anchor"),
        500.0,
        1e-6,
        "anchor elevation",
    );
    assert_eq!(at_center.elevation, None);
}

#[test]
fn zooming_keeps_the_anchored_ground_point_under_the_pointer() {
    for (pitch, pixel) in [(0.0, (600.0, 300.0)), (70.0, (900.0, 120.0))] {
        let style = style(false);
        let mut view = view(12.0, pitch);
        view.set_center_elevation(1500.0);
        let world = plateau(&style, &view, 1500.0);
        let pixel = Point2::new(pixel.0, pixel.1);
        let anchor = resolve_gesture_anchor(&style, &view, &world, pixel);
        let plane = anchor.elevation.expect("terrain anchor");
        let inverted = view.inverted_view_projection().expect("valid view");
        let before = view
            .window_to_world_at_elevation(&pixel.to_vec(), &inverted, plane)
            .expect("anchor unprojects");

        zoom_mercator_around(&mut view, anchor, Zoom::new(12.3));

        let inverted = view.inverted_view_projection().expect("valid view");
        let after = view
            .window_to_world_at_elevation(&pixel.to_vec(), &inverted, plane)
            .expect("anchor unprojects");
        let scale = 2_f64.powf(0.3);
        assert_close(after.x, before.x * scale, 1e-6, "anchor x");
        assert_close(after.y, before.y * scale, 1e-6, "anchor y");
    }
}

#[test]
fn zooming_without_terrain_anchors_on_the_ground_plane() {
    let mut view = view(12.0, 0.0);
    let pixel = Point2::new(600.0, 300.0);
    let inverted = view.inverted_view_projection().expect("valid view");
    let before = view
        .window_to_world_at_ground(&pixel.to_vec(), &inverted, false)
        .expect("ground unprojects");

    zoom_mercator_around(
        &mut view,
        GestureAnchor {
            pixel,
            elevation: None,
        },
        Zoom::new(13.0),
    );

    let inverted = view.inverted_view_projection().expect("valid view");
    let after = view
        .window_to_world_at_ground(&pixel.to_vec(), &inverted, false)
        .expect("ground unprojects");
    assert_close(after.x, before.x * 2.0, 1e-6, "anchor x");
    assert_close(after.y, before.y * 2.0, 1e-6, "anchor y");
}

#[test]
fn panning_moves_the_plane_point_with_the_cursor() {
    let mut view = view(12.0, 60.0);
    view.set_center_elevation(800.0);
    let cursor = Point2::new(700.0, 300.0);
    let delta = Vector2::new(40.0, -25.0);
    let inverted = view.inverted_view_projection().expect("valid view");
    let grabbed = view
        .window_to_world_at_elevation(&(cursor.to_vec() - delta), &inverted, 800.0)
        .expect("plane unprojects");

    pan_mercator_by_pixels(&mut view, cursor, delta, 800.0);

    let inverted = view.inverted_view_projection().expect("valid view");
    let under_cursor = view
        .window_to_world_at_elevation(&cursor.to_vec(), &inverted, 800.0)
        .expect("plane unprojects");
    assert_close(under_cursor.x, grabbed.x, 1e-6, "grabbed x");
    assert_close(under_cursor.y, grabbed.y, 1e-6, "grabbed y");
}

#[test]
fn recalculating_after_an_elevation_change_keeps_the_camera_in_place() {
    let mut view = view(12.0, 60.0);
    view.set_center_elevation(500.0);
    let (camera_before, altitude_before) = camera_ground_position(&view);
    let world_size_before = TILE_SIZE * 2_f64.powf(view.zoom().value());

    recalculate_zoom_and_center(&mut view, 1200.0);

    let (camera_after, altitude_after) = camera_ground_position(&view);
    let world_size_after = TILE_SIZE * 2_f64.powf(view.zoom().value());
    assert_close(view.center_elevation(), 1200.0, 1e-9, "center elevation");
    assert!(
        view.zoom().value() > 12.0,
        "a closer surface means a larger zoom"
    );
    assert_close(altitude_after, altitude_before, 0.5, "camera altitude");
    // The ground distance is converted with the pixel scale of the old center, as GL JS does,
    // so the camera drifts by the change of Mercator scale between the two centers only.
    assert_close(
        camera_after.x / world_size_after,
        camera_before.x / world_size_before,
        1e-6,
        "camera x",
    );
    assert_close(
        camera_after.y / world_size_after,
        camera_before.y / world_size_before,
        1e-6,
        "camera y",
    );
}

#[test]
fn finishing_a_gesture_thaws_the_elevation_and_reconciles_the_view() {
    let style = style(false);
    let mut view = view(12.0, 60.0);
    view.set_center_elevation(500.0);
    let world = plateau(&style, &view, 2000.0);
    view.freeze_center_elevation();

    finish_gesture(&style, &mut view, &world);

    assert!(!view.center_elevation_frozen());
    assert_close(view.center_elevation(), 2000.0, 1e-9, "center elevation");
    assert!(view.zoom().value() > 12.0);
}

#[test]
fn the_camera_ground_position_is_the_eye_of_the_view_matrix() {
    let mut view = view(12.0, 60.0);
    view.set_center_elevation(700.0);
    view.camera_mut().set_bearing(Deg(35.0));
    view.camera_mut().set_roll(Deg(20.0));

    let (position, altitude) = camera_ground_position(&view);
    let eye = view.eye_position();

    assert_close(position.x, eye.x, 1e-6, "camera x");
    assert_close(position.y, eye.y, 1e-6, "camera y");
    assert_close(altitude, eye.z, 1e-6, "camera altitude");
}

#[test]
fn a_camera_inside_the_terrain_is_lifted_above_it() {
    let style = style(false);
    let mut view = view(14.0, 80.0);
    view.set_center_elevation(0.0);
    let world = plateau(&style, &view, 3000.0);
    let (_, altitude_before) = camera_ground_position(&view);
    assert!(
        altitude_before < 3000.0,
        "the test camera starts inside the plateau"
    );
    let center_before = view.camera().position().to_vec() / TILE_SIZE / 2_f64.powf(14.0);

    assert!(keep_camera_above_terrain(&style, &mut view, &world));

    let (_, altitude_after) = camera_ground_position(&view);
    let center_after =
        view.camera().position().to_vec() / TILE_SIZE / 2_f64.powf(view.zoom().value());
    assert_close(altitude_after, 3000.0, 1e-6, "camera altitude");
    assert_close(center_after.x, center_before.x, 1e-9, "center x");
    assert_close(center_after.y, center_before.y, 1e-9, "center y");
    assert!(
        view.zoom().value() < 14.0,
        "the camera backs off by zooming out"
    );
    assert!(!keep_camera_above_terrain(&style, &mut view, &world));
}

#[test]
fn globe_rays_hit_the_plateau_at_its_elevation() {
    let style = style(true);
    let mut view = view(11.0, 0.0);
    view.set_center_elevation(1500.0);
    let world = plateau(&style, &view, 1500.0);

    let hit = screen_point_to_terrain(&style, &view, &world, Point2::new(600.0, 400.0))
        .expect("the center looks straight down at the plateau");

    assert_close(hit.elevation, 1500.0, 1e-3, "hit elevation");
    let world_size = TILE_SIZE * 2_f64.powf(11.0);
    let center = view.camera().position();
    assert_close(hit.mercator.x * world_size, center.x, 0.5, "hit x");
    assert_close(hit.mercator.y * world_size, center.y, 0.5, "hit y");
}

#[test]
fn globe_rays_into_the_sky_miss_the_terrain() {
    let style = style(true);
    let mut view = view(11.0, 85.0);
    view.set_center_elevation(1500.0);
    let world = plateau(&style, &view, 1500.0);

    let sky = screen_point_to_terrain(&style, &view, &world, Point2::new(600.0, 5.0));
    let ground = screen_point_to_terrain(&style, &view, &world, Point2::new(600.0, 790.0));

    assert!(sky.is_none());
    assert_close(
        ground
            .expect("the bottom of the screen looks at the plateau")
            .elevation,
        1500.0,
        1e-3,
        "ground elevation",
    );
}
