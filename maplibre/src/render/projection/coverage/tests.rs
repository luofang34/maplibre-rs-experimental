#![allow(clippy::expect_used, clippy::panic)]
#[test]
fn a_level_gaze_covers_the_ground_to_the_horizon_at_every_height() {
    use crate::render::view_state::ViewStatePadding;

    let style: crate::style::Style = serde_json::from_str(
        r#"{"version":8,"sources":{"dem":{"type":"raster-dem","tiles":["https://dem.example/{z}/{x}/{y}.png"],"tileSize":256,"maxzoom":12,"encoding":"terrarium"}},"layers":[],"terrain":{"source":"dem","exaggeration":1}}"#,
    )
    .expect("a terrain style parses");
    let world = crate::tcs::world::World::default();

    for altitude in [150.0, 4000.0, 1.0e6] {
        let eyed = pose_eye(altitude, 89.5, crate::projection::ProjectionType::Mercator);
        let level = eyed.zoom().zoom_level(crate::coords::TILE_SIZE);
        let region = super::view_region_for_projection(
            &style,
            &eyed,
            &world,
            level,
            ViewStatePadding::Tight,
        )
        .expect("covering")
        .expect("ground region");
        let tiles: Vec<_> = region.iter().collect();
        let eye = eyed.eye_position();
        let corners = eyed.frustum_corners();
        let world_size = crate::coords::TILE_SIZE * 2_f64.powf(eyed.zoom().value());
        for sample in 0..=20 {
            let share = f64::from(sample) / 20.0;
            let ray = corners[3] * (1.0 - share) + corners[2] * share - eye;
            let ground = eye + ray * (-eye.z / ray.z);
            assert!(
                tiles.iter().any(|tile| {
                    let size = world_size / 2_f64.powi(i32::from(u8::from(tile.z)));
                    (ground.x / size).floor() as i32 == tile.x
                        && (ground.y / size).floor() as i32 == tile.y
                }),
                "bottom ground missing at altitude {altitude}, sample {sample}"
            );
        }
    }
}

#[test]
fn a_level_gaze_on_the_globe_covers_the_ground_below_the_eye() {
    use crate::{coords::WorldTileCoords, render::view_state::ViewStatePadding};

    let style: crate::style::Style = serde_json::from_str(
        r#"{"version":8,"sources":{"dem":{"type":"raster-dem","tiles":["https://dem.example/{z}/{x}/{y}.png"],"tileSize":256,"maxzoom":12,"encoding":"terrarium"}},"layers":[],"terrain":{"source":"dem","exaggeration":1},"projection":{"type":"globe"}}"#,
    )
    .expect("a globe terrain style parses");
    let world = crate::tcs::world::World::default();
    let mut counts = Vec::new();
    // Pitches between 65 and 84 degrees are left out: the map's own camera, which the
    // fixture is built from, puts its center past the Mercator limit for them from this
    // height, which is a fixture limit, not an eye one. From 3000 km the horizon dips 47
    // degrees, below the bottom of an 80 degree field of view held level, so that gaze
    // holds only sky.
    for (altitude, pitch, sees_ground) in [
        (1.0e6, 30.0, true),
        (1.0e6, 85.0, true),
        (1.0e6, 89.5, true),
        (300_000.0, 89.5, true),
        (120_000.0, 89.0, true),
        (3.0e6, 89.5, false),
    ] {
        let eyed = pose_eye(altitude, pitch, crate::projection::ProjectionType::Globe);
        let level = eyed.zoom().zoom_level(crate::coords::TILE_SIZE);
        let region = super::view_region_for_projection(
            &style,
            &eyed,
            &world,
            level,
            ViewStatePadding::Tight,
        )
        .expect("the covering succeeds");
        let tiles: Vec<WorldTileCoords> = region.iter().flat_map(|region| region.iter()).collect();
        let finest = tiles.iter().map(|coords| u8::from(coords.z)).max();
        counts.push((altitude, pitch, eyed.zoom().value(), tiles.len(), finest));
        if !sees_ground {
            assert!(
                tiles.is_empty(),
                "a gaze holding only sky at {altitude} m draws {} tiles: {counts:?}",
                tiles.len()
            );
            continue;
        }
        // The ground the gaze meets is drawn at the detail the eye's height warrants: the
        // view level or the one below, spread over dozens of tiles, rather than a handful
        // of tiles levels coarser as a grazing gaze would be scored from an orbit camera.
        assert!(
            tiles.len() >= 25 && finest.is_some_and(|z| z + 1 >= u8::from(level)),
            "a gaze at pitch {pitch} on the globe at {altitude} m (zoom {:.1}, level {level:?}) draws {} tiles, finest {finest:?}: {counts:?}",
            eyed.zoom().value(),
            tiles.len()
        );
    }
}

fn pose_eye(
    altitude: f64,
    pitch: f64,
    projection: crate::projection::ProjectionType,
) -> crate::render::view_state::ViewState {
    use crate::{
        coords::{LatLon, WorldCoords, Zoom},
        render::view_state::{CameraPose, ViewState},
    };
    let eye_at = LatLon::new(47.26, 11.39);
    let zoom = Zoom::new(13.0);
    let mut own = ViewState::new(
        crate::window::PhysicalSize::new(1888, 1792).expect("a viewport"),
        WorldCoords::from_lat_lon(eye_at, zoom),
        zoom,
        cgmath::Deg(0.0),
        cgmath::Rad(1.4),
    );
    own.set_max_pitch(cgmath::Deg(180.0));
    own.set_camera_pose(CameraPose {
        position: eye_at,
        altitude_meters: altitude,
        bearing: cgmath::Deg(0.0),
        pitch: cgmath::Deg(pitch),
        roll: cgmath::Deg(0.0),
    });
    let mut eyed = ViewState::new(
        crate::window::PhysicalSize::new(1888, 1792).expect("a viewport"),
        WorldCoords::from_lat_lon(eye_at, zoom),
        zoom,
        cgmath::Deg(0.0),
        cgmath::Rad(1.4),
    );
    eyed.set_max_pitch(cgmath::Deg(89.0));
    let mut external = own.external_view();
    // Immersive near clipping must include the nearby ground, even at street height.
    external.frustum.near = 1e-4;
    eyed.set_external_view(external, &projection)
        .expect("the eye is accepted");
    eyed
}
