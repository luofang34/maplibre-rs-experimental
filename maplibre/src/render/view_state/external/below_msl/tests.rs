#![allow(clippy::expect_used, clippy::panic)]

use cgmath::{Deg, InnerSpace, Matrix3, Matrix4, Rad};

use crate::{
    coords::{LatLon, WorldCoords, Zoom, TILE_SIZE},
    projection::{
        body::Body,
        globe::{
            covering::{TileElevationProvider, TileElevationRange},
            covering_tiles::{
                covering_tiles_with_history, lod::LodContext, FarPlane, GlobeCoveringOptions,
                SourceZoomRange, ZoomRounding,
            },
        },
        ProjectionType,
    },
    render::{
        camera::EyeFrustum,
        projection::{globe_camera_for_view, projection_data_for_view, terrain_region},
        view_state::{ExternalAnchor, ExternalView, ViewState, ViewStatePadding},
    },
    style::Style,
    tcs::world::World,
    window::PhysicalSize,
};

const GLOBE: ProjectionType = ProjectionType::VerticalPerspective;

fn globe_terrain_style() -> Style {
    serde_json::from_str(
        r#"{
            "version": 8,
            "projection": { "type": "vertical-perspective" },
            "sources": {
                "dem": { "type": "raster-dem", "tiles": ["https://example.com/{z}/{x}/{y}.png"], "tileSize": 256 }
            },
            "terrain": { "source": "dem", "exaggeration": 1.0 },
            "layers": []
        }"#,
    )
    .expect("valid globe terrain style")
}

fn view_from_attitude(pitch_deg: f64, roll_deg: f64, heading_deg: f64) -> Matrix4<f64> {
    // Local frame: X east, Y north, Z up.
    // Zero attitude looks level north: right = +X (east), up = +Z (up), back = -Y (south).
    let base_from_local = Matrix3::new(
        1.0, 0.0, 0.0, // local X -> eye X
        0.0, 0.0, -1.0, // local Y -> eye -Z (forward)
        0.0, 1.0, 0.0, // local Z -> eye +Y (up)
    );
    let heading = Matrix3::from_angle_z(Deg(heading_deg));
    // Positive pitch looks up about eye right (+X); positive bank rolls clockwise about eye -Z.
    let pitch = Matrix3::from_angle_x(Deg(-pitch_deg));
    let bank = Matrix3::from_angle_z(Deg(roll_deg));
    Matrix4::from(bank * pitch * base_from_local * heading)
}

#[test]
fn external_view_below_msl_succeeds_across_all_attitudes() {
    let style = globe_terrain_style();
    let locations = [
        // Death Valley Badwater / Furnace Creek: -85.5 m MSL (0.5 m AGL)
        (LatLon::new(36.46, -116.86), -85.5),
        // Amsterdam Schiphol EHAM runway threshold: -3.0 m MSL
        (LatLon::new(52.3086, 4.7639), -3.0),
    ];
    let attitudes = [
        ("level", 0.0, 0.0),
        ("pitch +90", 90.0, 0.0),
        ("pitch -90", -90.0, 0.0),
        ("75 deg bank", -3.0, 75.0),
        ("+180 inverted", 0.0, 180.0),
        ("-180 inverted", 0.0, -180.0),
    ];

    for (position, altitude_meters) in locations {
        for (label, pitch_deg, roll_deg) in attitudes {
            let mut view_state = ViewState::new(
                PhysicalSize::new(1920, 1080).expect("viewport"),
                WorldCoords::from((0.0, 0.0)),
                Zoom::new(14.0),
                Deg(0.0),
                Rad(0.6435),
            );
            let external = ExternalView {
                anchor: ExternalAnchor {
                    position,
                    altitude_meters,
                },
                view: view_from_attitude(pitch_deg, roll_deg, 27.0),
                frustum: EyeFrustum::symmetric(Rad(0.75), 16.0 / 9.0, 0.5, 250_000.0),
            };
            view_state
                .set_external_view(external, &GLOBE)
                .unwrap_or_else(|e| {
                    panic!("set_external_view failed at {altitude_meters} m ({label}): {e:?}")
                });
            let camera = globe_camera_for_view(&view_state).unwrap_or_else(|e| {
                panic!("globe_camera_for_view failed at {altitude_meters} m ({label}): {e:?}")
            });
            assert!(
                camera
                    .clipping_plane()
                    .dot(camera.camera_position().extend(1.0))
                    > 0.0,
                "eye must sit on visible side of horizon at {altitude_meters} m ({label})"
            );
            projection_data_for_view(&style, &view_state).unwrap_or_else(|e| {
                panic!("projection_data_for_view failed at {altitude_meters} m ({label}): {e:?}")
            });
        }
    }
}

#[test]
fn external_view_retains_4000m_mountain_beyond_sea_level_horizon_and_handles_open_sky() {
    // At 300 m MSL, the sea-level horizon lies sqrt(2 * R * 300) ≈ 61.8 km away, whereas a
    // 4,000 m peak remains visible out to 61.8 + 225.8 ≈ 287.6 km. Place a 4,000 m peak 120 km
    // north of the observer (well past the 61.8 km sea-level horizon).
    let observer = LatLon::new(0.0, 0.0);
    let peak_distance_meters = 120_000.0;
    let peak_lat = (peak_distance_meters / Body::EARTH.radius_meters).to_degrees();

    let mut view_state = ViewState::new(
        PhysicalSize::new(1920, 1080).expect("viewport"),
        WorldCoords::from((0.0, 0.0)),
        Zoom::new(12.0),
        Deg(0.0),
        Rad(0.6435),
    );
    view_state
        .set_external_view(
            ExternalView {
                anchor: ExternalAnchor {
                    position: observer,
                    altitude_meters: 300.0,
                },
                view: view_from_attitude(0.0, 0.0, 0.0),
                frustum: EyeFrustum::symmetric(Rad(0.6), 16.0 / 9.0, 1.0, 350_000.0),
            },
            &GLOBE,
        )
        .expect("level 300 m external view");

    struct PeakElevation {
        peak_lat: f64,
    }
    impl TileElevationProvider for PeakElevation {
        fn elevation_range(&self, tile: crate::coords::TileCoords) -> TileElevationRange {
            let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
            let north = (std::f64::consts::PI * (1.0 - 2.0 * f64::from(tile.y) / scale))
                .sinh()
                .atan()
                .to_degrees();
            let south = (std::f64::consts::PI * (1.0 - 2.0 * f64::from(tile.y + 1) / scale))
                .sinh()
                .atan()
                .to_degrees();
            if (south..=north).contains(&self.peak_lat) {
                TileElevationRange {
                    min_meters: 0.0,
                    max_meters: 4_000.0,
                }
            } else {
                TileElevationRange {
                    min_meters: 0.0,
                    max_meters: 0.0,
                }
            }
        }
    }

    let camera = globe_camera_for_view(&view_state).expect("globe camera");
    let focal = view_state.external_lod_focal_pixels().expect("focal");
    let lod = LodContext::screen_resolution_focal(&camera, focal, 0.0, 0.0);
    let options = GlobeCoveringOptions {
        zoom: crate::coords::ZoomLevel::new(12),
        requested_zoom: 12.0,
        variable_zoom: true,
        rounding: ZoomRounding::Floor,
        zoom_range: SourceZoomRange::from_style(Some(0), Some(14)),
        padding: 0,
        max_tiles: 512,
    };
    let tiles = covering_tiles_with_history(
        &camera,
        options,
        (lod, FarPlane::HighestGround),
        &PeakElevation { peak_lat },
        None,
    )
    .expect("mountain covering");

    let peak_y = (1.0 - peak_lat.to_radians().tan().asinh() / std::f64::consts::PI) * 0.5;
    let retains_peak = tiles.iter().any(|tile| {
        let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
        (0.5 * scale).floor() as i32 == tile.x && (peak_y * scale).floor() as i32 == tile.y
    });
    assert!(
        retains_peak,
        "4,000 m peak 120 km away (beyond 61.8 km sea horizon) must be retained"
    );

    // Pure open-sky gaze (+75 deg pitch & +90 deg zenith, 0 m ground): 0 visible ground tiles, no error.
    for sky_pitch in [75.0, 90.0] {
        view_state
            .set_external_view(
                ExternalView {
                    anchor: ExternalAnchor {
                        position: observer,
                        altitude_meters: 300.0,
                    },
                    view: view_from_attitude(sky_pitch, 0.0, 0.0),
                    frustum: EyeFrustum::symmetric(Rad(0.5), 16.0 / 9.0, 1.0, 350_000.0),
                },
                &GLOBE,
            )
            .expect("open sky external view");
        let sky_camera = globe_camera_for_view(&view_state).expect("sky globe camera");
        let sky_lod = LodContext::screen_resolution_focal(&sky_camera, focal, 0.0, 0.0);
        let sky_tiles = covering_tiles_with_history(
            &sky_camera,
            options,
            (sky_lod, FarPlane::HighestGround),
            &TileElevationRange {
                min_meters: 0.0,
                max_meters: 0.0,
            },
            None,
        )
        .expect("open sky covering should not error");
        assert!(
            sky_tiles.is_empty(),
            "open sky at pitch {sky_pitch} expected 0 tiles, got {}",
            sky_tiles.len()
        );
    }
}

#[test]
fn three_degree_final_approach_at_15m_agl_selects_runway_zoom_from_lod_focal_pixels() {
    let style = globe_terrain_style();
    let threshold_origin = LatLon::new(52.3086, 4.7639);
    let mut view_state = ViewState::new(
        PhysicalSize::new(3840, 2160).expect("4K OTW viewport"),
        WorldCoords::from((0.0, 0.0)),
        Zoom::new(16.0),
        Deg(0.0),
        Rad(0.6435),
    );
    // 3-degree final approach at 15 m AGL with narrow conformal SVS / OTW vertical FOV (20 deg).
    let fovy = Rad(20.0_f64.to_radians());
    view_state
        .set_external_view(
            ExternalView {
                anchor: ExternalAnchor {
                    position: threshold_origin,
                    altitude_meters: 15.0,
                },
                view: view_from_attitude(-3.0, 0.0, 0.0),
                frustum: EyeFrustum::symmetric(fovy, 16.0 / 9.0, 0.5, 150_000.0),
            },
            &GLOBE,
        )
        .expect("3-degree glidepath external view");

    let focal = view_state.external_lod_focal_pixels().expect("focal");
    let region = terrain_region(
        &style,
        &view_state,
        &World::default(),
        256,
        ViewStatePadding::Tight,
    )
    .expect("terrain covering")
    .expect("visible region");

    for ahead_meters in [300.0, 600.0, 1_000.0] {
        let lat =
            threshold_origin.latitude + (ahead_meters / Body::EARTH.radius_meters).to_degrees();
        let x = threshold_origin.longitude / 360.0 + 0.5;
        let y = (1.0 - lat.to_radians().tan().asinh() / std::f64::consts::PI) * 0.5;
        let selected = region
            .iter()
            .filter(|tile| {
                let scale = 2_f64.powi(i32::from(u8::from(tile.z)));
                (x * scale).floor() as i32 == tile.x && (y * scale).floor() as i32 == tile.y
            })
            .map(|tile| u8::from(tile.z))
            .max()
            .unwrap_or_else(|| panic!("missing runway tile at {ahead_meters} m ahead"));
        let distance = ahead_meters.hypot(15.0)
            / Body::EARTH.circumference_at_latitude(threshold_origin.latitude);
        let expected_zoom = (focal / (TILE_SIZE * distance)).log2().floor() as u8;
        assert!(
            (18..=20).contains(&selected),
            "at {ahead_meters} m ahead expected z in 18..=20, got z={selected}"
        );
        assert!(
            selected.abs_diff(expected_zoom) <= 1,
            "at {ahead_meters} m ahead selected z={selected} vs focal expected z={expected_zoom}"
        );
    }
}

#[test]
fn external_view_crossing_sea_level_and_poles_keeps_covering_bounded() {
    use crate::render::{
        projection::view_region_for_projection, tile_view_pattern::DEFAULT_TILE_SIZE,
    };

    let style = globe_terrain_style();
    let world = World::default();
    let positions = [
        LatLon::new(39.1295, -75.4660), // KDOV coastal sea-level plain
        LatLon::new(52.3086, 4.7639),   // EHAM below-MSL threshold
        LatLon::new(89.9999, 0.0),      // near North Pole
        LatLon::new(90.0, 45.0),        // exact North Pole
        LatLon::new(-90.0, -135.0),     // exact South Pole
    ];
    let altitudes = [
        -3.0, -0.5, -0.01, -1e-4, -1e-9, 0.0, 1e-9, 1e-4, 0.005, 0.012, 0.5, 3.0,
    ];
    let attitudes = [
        ("level", 0.0, 0.0),
        ("touchdown flare", 5.0, -2.0),
        ("nadir", -90.0, 0.0),
    ];

    for position in positions {
        for altitude_meters in altitudes {
            for (label, pitch_deg, roll_deg) in attitudes {
                let mut view_state = ViewState::new(
                    PhysicalSize::new(2560, 1440).expect("viewport"),
                    WorldCoords::from((0.0, 0.0)),
                    Zoom::new(14.0),
                    Deg(0.0),
                    Rad(0.6435),
                );
                let external = ExternalView {
                    anchor: ExternalAnchor {
                        position,
                        altitude_meters,
                    },
                    view: view_from_attitude(pitch_deg, roll_deg, 14.0),
                    frustum: EyeFrustum::symmetric(Rad(0.75), 16.0 / 9.0, 0.5, 250_000.0),
                };
                view_state
                    .set_external_view(external, &GLOBE)
                    .unwrap_or_else(|e| {
                        panic!(
                            "set_external_view failed at {position:?} alt={altitude_meters} ({label}): {e:?}"
                        )
                    });
                assert!(
                    view_state.zoom().value().is_finite(),
                    "non-finite zoom at {position:?} alt={altitude_meters} ({label})"
                );
                let visible_level = view_state.zoom().zoom_level(DEFAULT_TILE_SIZE);
                assert!(
                    usize::from(u8::from(visible_level)) < crate::coords::MAX_ZOOM,
                    "zoom_level out of bounds at {position:?} alt={altitude_meters}: {visible_level:?}"
                );
                projection_data_for_view(&style, &view_state).unwrap_or_else(|e| {
                    panic!(
                        "projection_data_for_view failed at {position:?} alt={altitude_meters} ({label}): {e:?}"
                    )
                });
                for padding in [ViewStatePadding::Tight, ViewStatePadding::Loose] {
                    let region = view_region_for_projection(
                        &style,
                        &view_state,
                        &world,
                        visible_level,
                        padding,
                    )
                    .unwrap_or_else(|e| {
                        panic!(
                            "view_region_for_projection({padding:?}) failed at {position:?} alt={altitude_meters} ({label}): {e:?}"
                        )
                    });
                    if let Some(region) = region {
                        for coords in region.iter() {
                            assert!(
                                u8::from(coords.z) <= 22,
                                "view tile exceeded z=22 cap: {coords:?} at {position:?} alt={altitude_meters}"
                            );
                        }
                    }
                    terrain_region(&style, &view_state, &world, 256, padding).unwrap_or_else(|e| {
                        panic!(
                            "terrain_region({padding:?}) failed at {position:?} alt={altitude_meters} ({label}): {e:?}"
                        )
                    });
                }
            }
        }
    }
}
