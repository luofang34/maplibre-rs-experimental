#![allow(clippy::expect_used, clippy::panic)]
use super::*;
use crate::projection::body::Body;

#[test]
fn prefetch_limits_preserve_every_primary_tile() {
    let tiles: Vec<_> = (0..200)
        .map(|x| WorldTileCoords {
            x,
            y: 0,
            z: ZoomLevel::new(8),
        })
        .collect();
    let primary = ViewRegion::from_tiles(tiles.clone(), ZoomLevel::new(8), tiles.len());
    let secondary = ViewRegion::from_tiles(
        vec![WorldTileCoords {
            x: 200,
            y: 0,
            z: ZoomLevel::new(8),
        }],
        ZoomLevel::new(8),
        1,
    );
    let combined =
        union_regions(Some(primary), Some(secondary), ZoomLevel::new(8), 96).expect("a region");
    assert_eq!(combined.iter().collect::<Vec<_>>(), tiles);
}

#[test]
fn moving_eye_requests_start_with_all_visible_tiles() {
    use cgmath::{Deg, Matrix4, Rad, SquareMatrix, Vector3};

    use crate::{
        coords::{LatLon, WorldCoords, Zoom},
        render::{
            camera::EyeFrustum,
            view_state::{ExternalAnchor, ExternalView},
        },
        window::PhysicalSize,
    };
    let style: Style = serde_json::from_str(
        r#"{"version":8,"sources":{},"layers":[],"terrain":{"source":"dem"}}"#,
    )
    .expect("style");
    let mut world = World::default();
    for pitch in [60.0, 85.0, 90.0] {
        let mut view = ViewState::new(
            PhysicalSize::new(3840, 2160).expect("viewport"),
            WorldCoords::from((256.0, 256.0)),
            Zoom::new(10.0),
            Deg(0.0),
            Deg(45.0),
        );
        let mut destination = None;
        for height in [8000.0, 4000.0] {
            view.set_external_view(
                ExternalView {
                    anchor: ExternalAnchor {
                        position: LatLon::new(47.26, 11.39),
                        altitude_meters: 600.0,
                    },
                    view: (Matrix4::from_translation(Vector3::new(0.0, 0.0, height))
                        * Matrix4::from_angle_x(Deg(pitch)))
                    .invert()
                    .expect("eye"),
                    frustum: EyeFrustum::symmetric(Rad(1.4), 16.0 / 9.0, 0.05, 1.0e8),
                },
                &ProjectionType::Mercator,
            )
            .expect("pose");
            if destination.is_none() {
                destination = Some(view.clone());
            }
        }
        assert!(!view.eye_settled());
        view.set_request_overscan(1.5);
        assert_visible_requests(&style, &view, &world);
        world.resources.insert(PrefetchView {
            view_state: destination,
            placement: None,
        });
        assert_visible_requests(&style, &view, &world);
        world.resources.insert(PrefetchView::default());
    }
}

fn assert_visible_requests(style: &Style, view: &ViewState, world: &World) {
    let level = view.zoom().zoom_level(DEFAULT_TILE_SIZE);
    let visible = view_region_for_projection(style, view, world, level, ViewStatePadding::Tight)
        .expect("visible covering")
        .expect("visible region");
    let requests = view_region_for_projection(style, view, world, level, ViewStatePadding::Loose)
        .expect("request covering")
        .expect("request region");
    let visible: Vec<_> = visible.iter().collect();
    assert!(!visible.is_empty());
    assert_eq!(
        requests.iter().take(visible.len()).collect::<Vec<_>>(),
        visible
    );
}

#[test]
fn terrain_tiles_are_twice_the_dem_tile_and_floored_beyond_the_dem_source() {
    // GL JS TerrainTileManager: a 512-pixel DEM gives 1024-pixel terrain tiles, a level
    // below the 512-pixel view tiles, floored and free of the DEM source's own zoom range.
    let request = CoveringRequest::terrain(11.56, 512.0);
    assert_eq!(request.level, ZoomLevel::new(10));
    assert!((request.requested_zoom - 10.56).abs() < 1e-9);
    assert_eq!(request.rounding, ZoomRounding::Floor);
    assert_eq!(
        request.zoom_range,
        SourceZoomRange::from_style(Some(0), Some(22))
    );

    let small_dem = CoveringRequest::terrain(11.56, 256.0);
    assert_eq!(small_dem.level, ZoomLevel::new(11));
    assert_eq!(
        CoveringRequest::terrain(30.0, 512.0).level,
        ZoomLevel::new(22)
    );
    assert_eq!(
        CoveringRequest::terrain(0.4, 512.0).level,
        ZoomLevel::new(0)
    );
}

/// An eye 10 m above the sea off Montauk, looking level across Block Island Sound as a
/// free-globe pose with a 25 degree field: the case of a sunrise over the sea. The style
/// stands the terrain on a 256-pixel DEM.
pub(crate) fn sea_eye() -> (Style, ViewState) {
    use cgmath::{Deg, InnerSpace, Matrix3, Quaternion, Rotation};

    use crate::{
        coords::{LatLon, WorldCoords, Zoom},
        projection::globe::lat_lon_to_unit_sphere,
        render::view_state::{GlobePose, NavigationMode},
        window::PhysicalSize,
    };
    let style: Style = serde_json::from_str(
        r#"{"version":8,"layers":[],"terrain":{"source":"dem"},
            "sources":{"dem":{"type":"raster-dem","tiles":["dem/{z}/{x}/{y}.png"],"tileSize":256}},
            "projection":{"type":"vertical-perspective"}}"#,
    )
    .expect("style");
    let observer = LatLon::new(41.07204, -71.85733);
    let zoom = Zoom::new(16.0);
    let mut view = ViewState::new(
        PhysicalSize::new(1200, 900).expect("viewport"),
        WorldCoords::from_lat_lon(observer, zoom),
        zoom,
        Deg(0.0),
        Deg(25.0),
    );
    view.set_navigation_mode(
        NavigationMode::FreeGlobe,
        &ProjectionType::VerticalPerspective,
    )
    .expect("free navigation");
    let up = lat_lon_to_unit_sphere(observer);
    let longitude = observer.longitude.to_radians();
    let east = cgmath::Vector3::new(longitude.cos(), 0.0, -longitude.sin());
    let north = up.cross(east);
    let (sin, cos) = 58.4_f64.to_radians().sin_cos();
    let forward = east * sin + north * cos;
    let right = forward.cross(up).normalize();
    let camera_up = right.cross(forward);
    // The target sits 10 m up, 200 m ahead: the sea curves away 3 mm over that reach.
    let ahead = 200.0;
    let target = up * Body::EARTH.radius_meters + forward * ahead;
    let eye_to_body = Quaternion::from(Matrix3::from_cols(right, camera_up, -forward));
    let body_to_eye = eye_to_body.conjugate().normalize();
    view.set_globe_pose(GlobePose {
        target: target.normalize().into(),
        target_elevation_meters: 10.0,
        orientation: [
            body_to_eye.s,
            body_to_eye.v.x,
            body_to_eye.v.y,
            body_to_eye.v.z,
        ],
        distance_meters: ahead,
    })
    .expect("eye pose");
    assert!(
        (eye_to_body.rotate_vector(-cgmath::Vector3::unit_z()) - forward).magnitude() < 1e-9,
        "the pose looks along the bearing"
    );
    (style, view)
}

/// The level of the terrain tile drawn under the ground `meters` ahead of the sea eye, and
/// the level whose DEM texels match the screen's pixels there.
fn level_ahead(region: &ViewRegion, view: &ViewState, meters: f64) -> (Option<u8>, f64) {
    let observer = crate::coords::LatLon::new(41.07204, -71.85733);
    let radians = meters / Body::EARTH.radius_meters;
    let (sin, cos) = 58.4_f64.to_radians().sin_cos();
    let latitude = observer.latitude + (radians * cos).to_degrees();
    let longitude =
        observer.longitude + (radians * sin / observer.latitude.to_radians().cos()).to_degrees();
    let x = longitude / 360.0 + 0.5;
    let y = (1.0 - latitude.to_radians().tan().asinh() / std::f64::consts::PI) * 0.5;
    let drawn = region
        .iter()
        .filter(|tile| {
            let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
            (x * scale).floor() as i32 == tile.x && (y * scale).floor() as i32 == tile.y
        })
        .map(|tile| u8::from(tile.z))
        .max();
    let focal = 0.5 * view.height() / 12.5_f64.to_radians().tan();
    let distance = meters / Body::EARTH.circumference_at_latitude(observer.latitude);
    let request = CoveringRequest::terrain(view.zoom().value(), 256.0);
    let resolved =
        (focal / (TILE_SIZE * distance)).log2() + request.requested_zoom - view.zoom().value();
    (drawn, resolved)
}

#[test]
fn a_level_eye_draws_the_ground_at_and_past_its_sea_horizon_as_its_pixels_resolve() {
    let (style, view) = sea_eye();
    let region = terrain_region(
        &style,
        &view,
        &World::default(),
        256,
        ViewStatePadding::Tight,
    )
    .expect("covering")
    .expect("region");
    // The sea horizon of an eye 10 m up lies 11.3 km off. Its silhouette runs across the
    // line of sight, so foreshortening must not coarsen it into chords; Block Island, 25 km
    // off, rises above that horizon and must be drawn at all.
    for meters in [11_300.0, 25_000.0] {
        let (drawn, resolved) = level_ahead(&region, &view, meters);
        let drawn = drawn.unwrap_or_else(|| panic!("no terrain tile {meters} m ahead"));
        // The covering floors each tile's level, which alone may cost up to one.
        assert!(
            f64::from(drawn) >= resolved.floor() - 1.0,
            "{meters} m ahead the terrain is level {drawn}, the screen resolves {resolved:.2}"
        );
    }
    let count = region.iter().count();
    assert!(count <= 200, "the sea eye covers {count} tiles");
}

#[test]
fn a_level_eyes_view_tiles_keep_gl_js_levels() {
    use crate::projection::globe::covering_tiles::covering_tiles;
    let (style, view) = sea_eye();
    let world = World::default();
    let level = view.zoom().zoom_level(DEFAULT_TILE_SIZE);
    let mut drawn: Vec<_> =
        view_region_for_projection(&style, &view, &world, level, ViewStatePadding::Tight)
            .expect("covering")
            .expect("region")
            .iter()
            .collect();
    // Textures foreshorten with the ground they lie on, and fetching them at the surface's
    // detail would starve a tight memory budget's few requests of the drapes' own tiles.
    let camera = globe_camera_for_view(&view).expect("camera");
    let mut reference = covering_tiles(
        &camera,
        GlobeCoveringOptions {
            zoom: level,
            requested_zoom: view.zoom().value(),
            variable_zoom: u8::from(level) > 4,
            rounding: ZoomRounding::Floor,
            zoom_range: SourceZoomRange::default(),
            padding: 0,
            max_tiles: 512,
        },
        &TileElevationRange {
            min_meters: 0.0,
            max_meters: elevation_for_tile_culling(&camera, view.center_elevation()),
        },
    )
    .expect("reference covering");
    drawn.sort();
    reference.sort();
    assert_eq!(drawn, reference);
}
