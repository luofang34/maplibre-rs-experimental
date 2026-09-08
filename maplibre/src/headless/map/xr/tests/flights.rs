use super::*;

#[tokio::test]
async fn a_globe_on_a_table_requests_only_the_levels_it_shows() {
    use cgmath::Matrix3;

    let (width, height) = (1888, 1792);
    let (kernel, renderer) = create_headless_renderer(width, height, None)
        .await
        .expect("a headless renderer");
    let format = renderer.state().surface().surface_format();
    let mut map =
        HeadlessMap::new(terrain_globe_style(), renderer, kernel, device_plugins()).expect("a map");
    map.set_max_pitch(cgmath::Deg(89.0));
    let colors = [
        texture_sized(map.device(), format, width, height),
        texture_sized(map.device(), format, width, height),
    ];
    let depths = [
        texture_sized(
            map.device(),
            wgpu::TextureFormat::Depth32Float,
            width,
            height,
        ),
        texture_sized(
            map.device(),
            wgpu::TextureFormat::Depth32Float,
            width,
            height,
        ),
    ];
    // A globe fifteen centimetres in radius a metre ahead of the eyes, as on the table.
    let radius_meters = 6_371_008.8;
    let radius = 0.15;
    let scale = radius / radius_meters;
    let latitude = 47.26_f64.to_radians();
    let east = Vector3::new(1.0, 0.0, 0.0);
    let north = Vector3::new(0.0, latitude.cos(), -latitude.sin());
    let up = Vector3::new(0.0, latitude.sin(), latitude.cos());
    let rotation = Matrix3::from_cols(east, north, up);
    let center = Vector3::new(0.0, -0.3, -1.0);
    let world_from_scene = Matrix4::from_translation(center + up * radius)
        * Matrix4::from(rotation)
        * Matrix4::from_scale(scale);
    for frame_number in 0..2_u64 {
        let frame = XrFrame {
            opaque_environment: false,
            timestamp: Duration::from_millis(16 * (frame_number + 1)),
            placement: ScenePlacement {
                anchor: ExternalAnchor {
                    position: LatLon::new(47.26, 11.39),
                    altitude_meters: 0.0,
                },
                world_from_scene,
            },
            eyes: (0..2_u32)
                .map(|index| XrEye {
                    world_from_eye: Matrix4::from_translation(Vector3::new(
                        f64::from(index) * 0.064,
                        0.0,
                        0.0,
                    )),
                    frustum: EyeFrustum::symmetric(Rad(1.4), 1.05, 0.05, 100.0),
                    target: EyeTarget {
                        color: Some(colors[index as usize].create_view(&Default::default())),
                        depth: Some(depths[index as usize].create_view(&Default::default())),
                    },
                })
                .collect(),
            request_overscan: 1.2,
            prefetch: None,
        };
        map.run_xr_frame(frame)
            .expect("both eyes render the table globe");
    }
    let view = map.view_state();
    let level = u8::from(view.zoom().zoom_level(crate::coords::TILE_SIZE));
    let mut levels: Vec<u8> = map
        .world()
        .tiles
        .tiles
        .values()
        .map(|tile| u8::from(tile.coords.z))
        .collect();
    levels.sort_unstable();
    levels.dedup();
    assert!(
        levels.iter().all(|z| *z <= level + 2),
        "a table globe at zoom {:.2} (level {level}) requested tiles at levels {levels:?}",
        view.zoom().value()
    );
}

#[tokio::test]
async fn a_flight_from_the_table_requests_tiles_a_few_at_a_time() {
    use crate::io::tile_backpressure::{tiles_in_flight, MAX_TILES_IN_FLIGHT};
    use cgmath::Matrix3;

    let (width, height) = (1888, 1792);
    let (kernel, renderer) = create_headless_renderer(width, height, None)
        .await
        .expect("a headless renderer");
    let format = renderer.state().surface().surface_format();
    let mut map =
        HeadlessMap::new(terrain_globe_style(), renderer, kernel, device_plugins()).expect("a map");
    map.set_max_pitch(cgmath::Deg(89.0));
    let colors = [
        texture_sized(map.device(), format, width, height),
        texture_sized(map.device(), format, width, height),
    ];
    let depths = [
        texture_sized(
            map.device(),
            wgpu::TextureFormat::Depth32Float,
            width,
            height,
        ),
        texture_sized(
            map.device(),
            wgpu::TextureFormat::Depth32Float,
            width,
            height,
        ),
    ];
    let radius_meters = 6_371_008.8;
    let latitude = 47.26_f64.to_radians();
    let east = Vector3::new(1.0, 0.0, 0.0);
    let north = Vector3::new(0.0, latitude.cos(), -latitude.sin());
    let up = Vector3::new(0.0, latitude.sin(), latitude.cos());
    let rotation = Matrix3::from_cols(east, north, up);
    // The table globe the eyes see now, and the ground 2000 m under them the flight ends
    // at: the placement a frame carries as its prefetch.
    let table = Matrix4::from_translation(Vector3::new(0.0, -0.3, -1.0) + up * 0.15)
        * Matrix4::from(rotation)
        * Matrix4::from_scale(0.15 / radius_meters);
    let ground =
        Matrix4::from_translation(Vector3::new(0.0, -2000.0, 0.0)) * Matrix4::from(rotation);
    let anchor = ExternalAnchor {
        position: LatLon::new(47.26, 11.39),
        altitude_meters: 0.0,
    };
    for frame_number in 0..4_u64 {
        let frame = XrFrame {
            opaque_environment: false,
            timestamp: Duration::from_millis(16 * (frame_number + 1)),
            placement: ScenePlacement {
                anchor,
                world_from_scene: table,
            },
            eyes: (0..2_u32)
                .map(|index| XrEye {
                    world_from_eye: Matrix4::from_translation(Vector3::new(
                        f64::from(index) * 0.064,
                        0.0,
                        0.0,
                    )),
                    frustum: EyeFrustum::symmetric(Rad(1.4), 1.05, 0.05, 1.0e8),
                    target: EyeTarget {
                        color: Some(colors[index as usize].create_view(&Default::default())),
                        depth: Some(depths[index as usize].create_view(&Default::default())),
                    },
                })
                .collect(),
            request_overscan: 1.2,
            prefetch: Some(ScenePlacement {
                anchor,
                world_from_scene: ground,
            }),
        };
        map.run_xr_frame(frame).expect("the frame renders");
        // No tile ever arrives here, so every request stays in flight.
        let in_flight = tiles_in_flight(&map.world().tiles);
        assert!(
            in_flight <= MAX_TILES_IN_FLIGHT,
            "frame {frame_number}: {in_flight} tiles in flight, {} resident",
            map.world().tiles.tiles.len()
        );
    }
}

#[tokio::test]
async fn a_flights_prefetch_is_built_once_and_does_not_follow_the_head() {
    use crate::render::xr::PrefetchView;
    use cgmath::Matrix3;

    let (width, height) = (1888, 1792);
    let (kernel, renderer) = create_headless_renderer(width, height, None)
        .await
        .expect("a headless renderer");
    let format = renderer.state().surface().surface_format();
    let mut map =
        HeadlessMap::new(terrain_globe_style(), renderer, kernel, device_plugins()).expect("a map");
    map.set_max_pitch(cgmath::Deg(89.0));
    let colors = [
        texture_sized(map.device(), format, width, height),
        texture_sized(map.device(), format, width, height),
    ];
    let depths = [
        texture_sized(
            map.device(),
            wgpu::TextureFormat::Depth32Float,
            width,
            height,
        ),
        texture_sized(
            map.device(),
            wgpu::TextureFormat::Depth32Float,
            width,
            height,
        ),
    ];
    let radius_meters = 6_371_008.8;
    let latitude = 47.26_f64.to_radians();
    let rotation = Matrix3::from_cols(
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, latitude.cos(), -latitude.sin()),
        Vector3::new(0.0, latitude.sin(), latitude.cos()),
    );
    let table = Matrix4::from_translation(Vector3::new(0.0, -0.15, -1.0))
        * Matrix4::from(rotation)
        * Matrix4::from_scale(0.15 / radius_meters);
    let ground =
        Matrix4::from_translation(Vector3::new(0.0, -2000.0, 0.0)) * Matrix4::from(rotation);
    let anchor = ExternalAnchor {
        position: LatLon::new(47.26, 11.39),
        altitude_meters: 0.0,
    };
    let mut centers = Vec::new();
    for (frame_number, yaw) in [0.0_f64, 25.0, -40.0].into_iter().enumerate() {
        // The anchor's altitude follows the terrain as it loads during the flight.
        let anchor = ExternalAnchor {
            altitude_meters: 100.0 * frame_number as f64,
            ..anchor
        };
        let frame = XrFrame {
            opaque_environment: false,
            timestamp: Duration::from_millis(16 * (frame_number as u64 + 1)),
            placement: ScenePlacement {
                anchor,
                world_from_scene: table,
            },
            eyes: (0..2_u32)
                .map(|index| XrEye {
                    // The head turns from frame to frame while the flight runs.
                    world_from_eye: Matrix4::from_angle_y(cgmath::Deg(yaw))
                        * Matrix4::from_translation(Vector3::new(
                            f64::from(index) * 0.064,
                            0.0,
                            0.0,
                        )),
                    frustum: EyeFrustum::symmetric(Rad(1.4), 1.05, 0.05, 1.0e8),
                    target: EyeTarget {
                        color: Some(colors[index as usize].create_view(&Default::default())),
                        depth: Some(depths[index as usize].create_view(&Default::default())),
                    },
                })
                .collect(),
            request_overscan: 1.2,
            prefetch: Some(ScenePlacement {
                anchor,
                world_from_scene: ground,
            }),
        };
        map.run_xr_frame(frame).expect("the frame renders");
        let prefetch = map
            .world()
            .resources
            .get::<PrefetchView>()
            .and_then(|prefetch| prefetch.view_state.as_ref())
            .expect("a flight keeps its prefetch view");
        let view = prefetch.external_view();
        centers.push((
            view.anchor.position.latitude,
            view.anchor.position.longitude,
            prefetch.zoom().value(),
        ));
    }
    assert!(
        centers.windows(2).all(|pair| pair[0] == pair[1]),
        "the prefetch view followed the head: {centers:?}"
    );
}

#[tokio::test]
async fn terrain_draws_reuse_their_bind_groups_from_frame_to_frame() {
    use crate::{
        render::eventually::{Eventually, Eventually::Initialized},
        terrain::resources::TerrainResources,
    };
    use cgmath::Matrix3;

    let (width, height) = (1888, 1792);
    let (kernel, renderer) = create_headless_renderer(width, height, None)
        .await
        .expect("a headless renderer");
    let format = renderer.state().surface().surface_format();
    let mut map =
        HeadlessMap::new(terrain_globe_style(), renderer, kernel, device_plugins()).expect("a map");
    let colors = [
        texture_sized(map.device(), format, width, height),
        texture_sized(map.device(), format, width, height),
    ];
    let depths = [
        texture_sized(
            map.device(),
            wgpu::TextureFormat::Depth32Float,
            width,
            height,
        ),
        texture_sized(
            map.device(),
            wgpu::TextureFormat::Depth32Float,
            width,
            height,
        ),
    ];
    let radius_meters = 6_371_008.8;
    let latitude = 47.26_f64.to_radians();
    let rotation = Matrix3::from_cols(
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, latitude.cos(), -latitude.sin()),
        Vector3::new(0.0, latitude.sin(), latitude.cos()),
    );
    let table = Matrix4::from_translation(Vector3::new(0.0, -0.15, -1.0))
        * Matrix4::from(rotation)
        * Matrix4::from_scale(0.15 / radius_meters);
    let mut groups_per_frame = Vec::new();
    for frame_number in 0..3_u64 {
        let frame = XrFrame {
            opaque_environment: false,
            timestamp: Duration::from_millis(16 * (frame_number + 1)),
            placement: ScenePlacement {
                anchor: ExternalAnchor {
                    position: LatLon::new(47.26, 11.39),
                    altitude_meters: 0.0,
                },
                world_from_scene: table,
            },
            eyes: (0..2_u32)
                .map(|index| XrEye {
                    world_from_eye: Matrix4::from_translation(Vector3::new(
                        f64::from(index) * 0.064,
                        0.0,
                        0.0,
                    )),
                    frustum: EyeFrustum::symmetric(Rad(1.4), 1.05, 0.05, 100.0),
                    target: EyeTarget {
                        color: Some(colors[index as usize].create_view(&Default::default())),
                        depth: Some(depths[index as usize].create_view(&Default::default())),
                    },
                })
                .collect(),
            request_overscan: 1.2,
            prefetch: None,
        };
        map.run_xr_frame(frame).expect("the frame renders");
        let Some(Initialized(terrain)) =
            map.world().resources.get::<Eventually<TerrainResources>>()
        else {
            panic!("terrain resources exist");
        };
        let mut groups: Vec<_> = terrain
            .draws()
            .iter()
            .map(|draw| (draw.coords, draw.bind_group.global_id()))
            .collect();
        groups.sort_by_key(|(coords, _)| (u8::from(coords.z), coords.x, coords.y));
        groups_per_frame.push(groups);
    }
    assert!(
        !groups_per_frame[2].is_empty(),
        "the table globe draws terrain tiles"
    );
    assert_eq!(
        groups_per_frame[1], groups_per_frame[2],
        "a frame at rest recreated its terrain bind groups"
    );
}
