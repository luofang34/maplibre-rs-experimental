use std::mem::{align_of, size_of};

use cgmath::{Matrix4, Vector4};

use super::ShaderProjectionData;
use crate::{
    projection::renderer_data::RendererProjectionData, render::shaders::ShaderTileMetadata,
};

#[test]
fn shader_projection_data_has_uniform_safe_layout() {
    assert_eq!(size_of::<ShaderProjectionData>(), 96);
    assert_eq!(align_of::<ShaderProjectionData>(), 4);
    assert_eq!(size_of::<ShaderTileMetadata>(), 96);
}

#[test]
fn renderer_projection_data_preserves_shader_values() {
    let data = RendererProjectionData {
        main_matrix: Matrix4::from_scale(2.0),
        tile_mercator_coords: Vector4::new(0.25, 0.5, 0.125, 0.125),
        clipping_plane: Vector4::new(1.0, 2.0, 3.0, 4.0),
        projection_transition: 0.75,
        fallback_matrix: Matrix4::from_scale(3.0),
        clip_antimeridian: true,
    };

    let shader = ShaderProjectionData::from_renderer_data(data);
    let expected_matrix: [[f32; 4]; 4] = data.main_matrix.into();
    let expected_plane: [f32; 4] = data.clipping_plane.into();

    assert_eq!(shader.main_matrix, expected_matrix);
    assert_eq!(shader.clipping_plane, expected_plane);
    assert_eq!(shader.transition, 0.75);
}
#[test]
fn mercator_projection_uses_inert_shader_state() {
    let style = crate::style::Style::default();
    let view = crate::render::view_state::ViewState::new(
        crate::window::PhysicalSize::new(512, 512).expect("test viewport should be valid"),
        crate::coords::WorldCoords::from((256.0, 256.0)),
        crate::coords::Zoom::default(),
        cgmath::Deg(0.0),
        cgmath::Deg(45.0),
    );

    let shader = super::projection_data_for_view(&style, &view)
        .expect("Mercator projection state should be valid");

    assert_eq!(shader.transition, 0.0);
    assert_eq!(shader.clipping_plane, [0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn globe_projection_builds_active_shader_state() {
    let style = crate::style::Style {
        projection: Some(crate::projection::ProjectionSpecification {
            projection_type: crate::projection::ProjectionType::Globe,
        }),
        ..Default::default()
    };
    let view = crate::render::view_state::ViewState::new(
        crate::window::PhysicalSize::new(800, 600).expect("test viewport should be valid"),
        crate::coords::WorldCoords::from((256.0, 256.0)),
        crate::coords::Zoom::default(),
        cgmath::Deg(0.0),
        cgmath::Deg(45.0),
    );

    let shader = super::projection_data_for_view(&style, &view)
        .expect("globe projection state should be valid");

    assert_eq!(shader.transition, 1.0);
    assert!(shader.main_matrix.into_iter().flatten().all(f32::is_finite));
    assert!(shader.clipping_plane.into_iter().all(f32::is_finite));
}

#[test]
fn globe_shorthand_transitions_to_mercator_at_high_zoom() {
    let style = crate::style::Style {
        projection: Some(crate::projection::ProjectionSpecification {
            projection_type: crate::projection::ProjectionType::Globe,
        }),
        ..Default::default()
    };
    let transition_view = crate::render::view_state::ViewState::new(
        crate::window::PhysicalSize::new(800, 600).expect("test viewport should be valid"),
        crate::coords::WorldCoords::from((
            crate::coords::TILE_SIZE * 2.0_f64.powf(11.5) * 0.5,
            crate::coords::TILE_SIZE * 2.0_f64.powf(11.5) * 0.5,
        )),
        crate::coords::Zoom::new(11.5),
        cgmath::Deg(0.0),
        cgmath::Deg(45.0),
    );
    let mercator_view = crate::render::view_state::ViewState::new(
        crate::window::PhysicalSize::new(800, 600).expect("test viewport should be valid"),
        crate::coords::WorldCoords::from((2_097_152.0, 2_097_152.0)),
        crate::coords::Zoom::new(12.0),
        cgmath::Deg(0.0),
        cgmath::Deg(45.0),
    );

    let transition = super::projection_data_for_view(&style, &transition_view)
        .expect("transition projection should be valid");
    let mercator = super::projection_data_for_view(&style, &mercator_view)
        .expect("high-zoom projection should be valid");
    let identity: [[f32; 4]; 4] = cgmath::Matrix4::from_scale(1.0).into();

    assert_eq!(transition.transition, 0.5);
    assert_eq!(mercator.transition, 0.0);
    assert_eq!(mercator.main_matrix, identity);
}
#[test]
fn globe_view_region_uses_reference_covering_tiles() {
    let style = crate::style::Style {
        projection: Some(crate::projection::ProjectionSpecification {
            projection_type: crate::projection::ProjectionType::Globe,
        }),
        ..Default::default()
    };
    let zoom_level = crate::coords::ZoomLevel::new(3);
    let world_size = crate::coords::TILE_SIZE * 8.0;
    let longitude = -0.02_f64;
    let latitude = 0.01_f64.to_radians();
    let view = crate::render::view_state::ViewState::new(
        crate::window::PhysicalSize::new(128, 128).expect("test viewport should be valid"),
        crate::coords::WorldCoords::from((
            (longitude / 360.0 + 0.5) * world_size,
            (1.0 - latitude.tan().asinh() / std::f64::consts::PI) * 0.5 * world_size,
        )),
        crate::coords::Zoom::from(zoom_level),
        cgmath::Deg(0.0),
        cgmath::Deg(36.869_897_645_844_02),
    );

    let region = super::view_region_for_projection(
        &style,
        &view,
        &crate::tcs::world::World::default(),
        zoom_level,
        crate::render::view_state::ViewStatePadding::Tight,
    )
    .expect("globe covering should succeed")
    .expect("globe covering always produces an explicit region");
    let expected = vec![
        (3, 3, zoom_level).into(),
        (3, 4, zoom_level).into(),
        (4, 3, zoom_level).into(),
        (4, 4, zoom_level).into(),
    ];

    assert_eq!(region.iter().collect::<Vec<_>>(), expected);
}
#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn projection_aware_tile_pipelines_compile() {
    use crate::render::{
        resource::{RenderPipeline, TilePipeline},
        settings::RendererSettings,
        shaders::{
            AtmosphereShader, FillShader, GlobeBackgroundShader, LineShader, RasterShader, Shader,
            SymbolShader, TileMaskShader,
        },
    };

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
    let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, None)
        .await
        .expect("GPU adapter should be available");
    let (device, _) = adapter
        .request_device(&wgpu::DeviceDescriptor::default(), None)
        .await
        .expect("GPU device should be available");
    let projection = super::ProjectionGpuResources::new(&device);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let shaders: [(&str, Box<dyn Shader>, bool, bool); 7] = [
        ("test fill", Box::new(FillShader { format }), false, false),
        ("test line", Box::new(LineShader { format }), false, false),
        (
            "test mask",
            Box::new(TileMaskShader {
                format,
                draw_colors: false,
                debug_lines: false,
            }),
            false,
            false,
        ),
        (
            "test raster",
            Box::new(RasterShader { format }),
            true,
            false,
        ),
        (
            "test symbol",
            Box::new(SymbolShader { format }),
            false,
            true,
        ),
        (
            "test globe background",
            Box::new(GlobeBackgroundShader { format }),
            false,
            false,
        ),
        (
            "test atmosphere",
            Box::new(AtmosphereShader { format }),
            false,
            false,
        ),
    ];

    for (name, shader, raster, glyph) in shaders {
        let mut descriptor = TilePipeline::new(
            name.into(),
            RendererSettings::default(),
            shader.describe_vertex(),
            shader.describe_fragment(),
            true,
            false,
            false,
            false,
            false,
            raster,
            glyph,
        )
        .describe_render_pipeline();
        if glyph {
            // Symbols take the terrain tile they stand on at group 2.
            descriptor
                .layout
                .get_or_insert_with(Vec::new)
                .push(crate::terrain::resources::TerrainResources::bind_group_layout_entries());
        }
        descriptor.initialize_with_prefix_layouts(&device, &[projection.bind_group_layout()]);
    }
}

#[test]
fn the_projection_uniform_carries_the_body_radius() {
    let style = crate::style::Style::default();
    let mut view = crate::render::view_state::ViewState::new(
        crate::window::PhysicalSize::new(512, 512).expect("test viewport should be valid"),
        crate::coords::WorldCoords::from((256.0, 256.0)),
        crate::coords::Zoom::default(),
        cgmath::Deg(0.0),
        cgmath::Deg(45.0),
    );
    let earth = super::projection_data_for_view(&style, &view).expect("valid state");
    view.set_body(crate::projection::body::Body {
        radius_meters: 1_737_400.0,
    });
    let moon = super::projection_data_for_view(&style, &view).expect("valid state");

    assert_eq!(earth.radius_meters, 6_371_008.8_f32);
    assert_eq!(moon.radius_meters, 1_737_400.0);
}

/// Whether `tile`, at its own level, contains `point_tile` at a finer or equal level.
fn tile_covers(
    tile: crate::coords::WorldTileCoords,
    point_tile: crate::coords::WorldTileCoords,
) -> bool {
    let (z, point_z) = (u8::from(tile.z), u8::from(point_tile.z));
    if z > point_z {
        return false;
    }
    let shift = point_z - z;
    tile.x == point_tile.x >> shift && tile.y == point_tile.y >> shift
}

#[test]
fn an_eyes_requests_cover_the_ground_behind_it() {
    use crate::{
        coords::{LatLon, WorldCoords, Zoom},
        render::view_state::{CameraPose, ViewState, ViewStatePadding},
    };

    let style: crate::style::Style = serde_json::from_str(
        r#"{"version":8,"sources":{"dem":{"type":"raster-dem","tiles":["https://dem.example/{z}/{x}/{y}.png"],"tileSize":256,"maxzoom":12,"encoding":"terrarium"}},"layers":[],"terrain":{"source":"dem","exaggeration":1}}"#,
    )
    .expect("a terrain style parses");
    let zoom = Zoom::new(13.0);
    let eye_at = LatLon::new(47.26, 11.39);
    let mut own = ViewState::new(
        crate::window::PhysicalSize::new(800, 600).expect("a viewport"),
        WorldCoords::from_lat_lon(eye_at, zoom),
        zoom,
        cgmath::Deg(0.0),
        cgmath::Rad(0.6435011087932844),
    );
    own.set_max_pitch(cgmath::Deg(180.0));
    // Looking north, sixty degrees down from level, two kilometres up.
    own.set_camera_pose(CameraPose {
        position: eye_at,
        altitude_meters: 2000.0,
        bearing: cgmath::Deg(0.0),
        pitch: cgmath::Deg(60.0),
        roll: cgmath::Deg(0.0),
    });
    let mut eyed = ViewState::new(
        crate::window::PhysicalSize::new(800, 600).expect("a viewport"),
        WorldCoords::from_lat_lon(eye_at, zoom),
        zoom,
        cgmath::Deg(0.0),
        cgmath::Rad(0.6435011087932844),
    );
    eyed.set_external_view(
        own.external_view(),
        &crate::projection::ProjectionType::Mercator,
    )
    .expect("the eye is accepted");
    let level = eyed.zoom().zoom_level(crate::coords::TILE_SIZE);
    let world = crate::tcs::world::World::default();
    let region = |padding| {
        super::view_region_for_projection(&style, &eyed, &world, level, padding)
            .expect("the covering succeeds")
            .expect("a frustum covering is explicit")
            .iter()
            .collect::<Vec<_>>()
    };

    // Three kilometres south of the eye, behind its back.
    let behind = LatLon::new(eye_at.latitude - 0.027, eye_at.longitude);
    let behind_tile = WorldCoords::from_lat_lon(behind, zoom).into_world_tile(level, zoom);
    let drawn = region(ViewStatePadding::Tight);
    let requested = region(ViewStatePadding::Loose);
    assert!(
        !drawn.iter().any(|tile| tile_covers(*tile, behind_tile)),
        "the frame does not show what is behind the eye"
    );
    assert!(
        requested.iter().any(|tile| tile_covers(*tile, behind_tile)),
        "the requests reach behind the eye: {} tiles, none covers {behind_tile}",
        requested.len()
    );
    assert!(
        requested.len() <= 1024,
        "the surround stays within the request cap: {}",
        requested.len()
    );
    // The requests hold everything the frame draws.
    for tile in &drawn {
        assert!(
            requested
                .iter()
                .any(|candidate| tile_covers(*candidate, *tile)),
            "{tile} is drawn but not requested"
        );
    }
}

#[test]
fn a_level_gaze_covers_the_ground_to_the_horizon_at_every_height() {
    use crate::{
        coords::{LatLon, WorldCoords, Zoom},
        render::view_state::{CameraPose, ViewState, ViewStatePadding},
    };

    let style: crate::style::Style = serde_json::from_str(
        r#"{"version":8,"sources":{"dem":{"type":"raster-dem","tiles":["https://dem.example/{z}/{x}/{y}.png"],"tileSize":256,"maxzoom":12,"encoding":"terrarium"}},"layers":[],"terrain":{"source":"dem","exaggeration":1}}"#,
    )
    .expect("a terrain style parses");
    let eye_at = LatLon::new(47.26, 11.39);
    let world = crate::tcs::world::World::default();
    let mut counts = Vec::new();
    for altitude in [150.0, 4000.0, 1.0e6] {
        let zoom = Zoom::new(13.0);
        let mut own = ViewState::new(
            crate::window::PhysicalSize::new(1888, 1792).expect("a viewport"),
            WorldCoords::from_lat_lon(eye_at, zoom),
            zoom,
            cgmath::Deg(0.0),
            cgmath::Rad(1.4),
        );
        own.set_max_pitch(cgmath::Deg(180.0));
        // Half a degree below level, as a head looking at the horizon.
        own.set_camera_pose(CameraPose {
            position: eye_at,
            altitude_meters: altitude,
            bearing: cgmath::Deg(0.0),
            pitch: cgmath::Deg(89.5),
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
        eyed.set_external_view(
            own.external_view(),
            &crate::projection::ProjectionType::Mercator,
        )
        .expect("the eye is accepted");
        let level = eyed.zoom().zoom_level(crate::coords::TILE_SIZE);
        let drawn = super::view_region_for_projection(
            &style,
            &eyed,
            &world,
            level,
            ViewStatePadding::Tight,
        )
        .expect("the covering succeeds")
        .map_or(0, |region| region.iter().count());
        counts.push((altitude, eyed.zoom().value(), drawn));
    }
    for (altitude, zoom, drawn) in &counts {
        assert!(
            *drawn >= 30,
            "a level gaze at {altitude} m (zoom {zoom:.1}) draws only {drawn} tiles: {counts:?}"
        );
    }
}

#[test]
fn a_level_gaze_on_the_globe_covers_the_ground_below_the_eye() {
    use crate::{
        coords::{LatLon, WorldCoords, WorldTileCoords, Zoom},
        render::view_state::{CameraPose, ViewState, ViewStatePadding},
    };

    let style: crate::style::Style = serde_json::from_str(
        r#"{"version":8,"sources":{"dem":{"type":"raster-dem","tiles":["https://dem.example/{z}/{x}/{y}.png"],"tileSize":256,"maxzoom":12,"encoding":"terrarium"}},"layers":[],"terrain":{"source":"dem","exaggeration":1},"projection":{"type":"globe"}}"#,
    )
    .expect("a globe terrain style parses");
    let eye_at = LatLon::new(47.26, 11.39);
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
        let zoom = Zoom::new(5.0);
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
        eyed.set_external_view(
            own.external_view(),
            &crate::projection::ProjectionType::Globe,
        )
        .expect("the eye is accepted");
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
