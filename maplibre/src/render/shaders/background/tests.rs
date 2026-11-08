use cgmath::Point2;

use super::{AtmosphereLayerMetadata, SkyLayerMetadata, SkyShader};
use crate::{
    coords::LatLon,
    projection::{
        body::Body,
        globe::{
            camera::{GlobeCameraOptions, GlobeCameraState},
            lat_lon_to_unit_sphere,
        },
    },
    render::SceneLighting,
    style::light::LightSpecification,
};

fn camera() -> GlobeCameraState {
    GlobeCameraState::new(GlobeCameraOptions {
        width: 512.0,
        height: 512.0,
        field_of_view_degrees: 45.0,
        center: LatLon::new(0.0, 160.0),
        world_size: 1024.0,
        bearing_degrees: 20.0,
        pitch_degrees: 30.0,
        roll_degrees: 0.0,
        center_offset: Point2::new(0.0, 0.0),

        body: Body::EARTH,
        target_elevation_meters: 0.0,
        radius_pixels: None,
    })
    .expect("camera should be valid")
}

#[test]
fn atmosphere_metadata_matches_gpu_vertex_layout() {
    assert_eq!(std::mem::size_of::<AtmosphereLayerMetadata>(), 112);

    let light: LightSpecification =
        serde_json::from_str(r#"{"anchor":"map","position":[1.5,90,90]}"#)
            .expect("light should parse");
    let metadata =
        AtmosphereLayerMetadata::from_view(&camera(), &light, Default::default(), 1.0, 0.75)
            .expect("metadata should be valid");

    assert_eq!(metadata.radius_blend_padding[1], 0.75);
    assert!(metadata.radius_blend_padding[0] > 0.0);
    assert!(metadata.globe_position[..3]
        .iter()
        .all(|value| value.is_finite()));
    assert!(metadata.sun_direction[..3]
        .iter()
        .all(|value| value.is_finite()));
    assert!(metadata
        .inverse_projection
        .iter()
        .flatten()
        .all(|value| value.is_finite()));
}

#[test]
fn disabled_metadata_cannot_contribute_color() {
    let metadata = AtmosphereLayerMetadata::disabled();
    assert_eq!(
        metadata.radius_blend_padding,
        [1.0, 0.0, Body::EARTH.radius_meters as f32, 0.0]
    );
}

#[test]
fn host_sun_replaces_the_style_light() {
    let camera = camera();
    let light: LightSpecification =
        serde_json::from_str(r#"{"anchor":"map","position":[1.5,90,90]}"#)
            .expect("light should parse");
    // The sun stands over the camera's own center, 0°N 160°E.
    let sun = lat_lon_to_unit_sphere(LatLon::new(0.0, 160.0));
    let expected = camera.world_direction_to_view(sun).expect("view direction");

    let from_host = AtmosphereLayerMetadata::from_view(
        &camera,
        &light,
        SceneLighting {
            sun_direction: Some(sun),
            ..Default::default()
        },
        1.0,
        1.0,
    )
    .expect("metadata should be valid");
    let from_style =
        AtmosphereLayerMetadata::from_view(&camera, &light, Default::default(), 1.0, 1.0)
            .expect("metadata should be valid");

    for (axis, expected) in [expected.x, expected.y, expected.z].into_iter().enumerate() {
        assert!((f64::from(from_host.sun_direction[axis]) - expected).abs() < 1e-6);
    }
    assert_ne!(from_host.sun_direction, from_style.sun_direction);
}

#[test]
fn sky_metadata_matches_its_vertex_layout() {
    use crate::render::{resource::VertexBufferLayout, shaders::Shader};
    assert_eq!(std::mem::size_of::<SkyLayerMetadata>(), 9 * 16);
    let shader = SkyShader {
        format: wgpu::TextureFormat::Rgba8Unorm,
    };
    let vertex = shader.describe_vertex();
    let [VertexBufferLayout {
        array_stride,
        attributes,
        ..
    }] = vertex.buffers.as_slice()
    else {
        panic!("the sky reads one instance buffer");
    };
    assert_eq!(*array_stride, 9 * 16);
    let mut locations: Vec<u32> = attributes.iter().map(|a| a.shader_location).collect();
    locations.sort_unstable();
    // WebGPU allows locations below 16 only.
    assert_eq!(locations, (7..16).collect::<Vec<_>>());
    let up = attributes.iter().find(|a| a.shader_location == 7);
    assert_eq!(up.map(|a| a.offset), Some(8 * 16));
}

#[test]
fn sky_metadata_scales_horizon_and_blend_to_physical_frame_height() {
    use cgmath::{Deg, Rad};

    use crate::{
        background::buffers::sky_metadata,
        coords::{WorldCoords, Zoom},
        render::view_state::ViewState,
        style::sky::SkySpecification,
        window::{LogicalSize, PhysicalSize},
    };

    let mut view = ViewState::new(
        PhysicalSize::new(1600, 840).expect("physical size"),
        WorldCoords::from_lat_lon(LatLon::new(47.27574, 11.39085), Zoom::new(12.0)),
        Zoom::new(12.0),
        Deg(0.0),
        Rad(0.6435011087932844),
    );
    view.set_max_pitch(Deg(85.0));
    view.camera_mut().set_pitch(Deg(70.0));
    view.resize(LogicalSize::new(800, 420).expect("logical size"));

    let sky = SkySpecification::default();
    let one_x = sky_metadata(&sky, &view, 0.0, 420.0);
    let two_x = sky_metadata(&sky, &view, 0.0, 840.0);

    assert!((two_x.blend[2] - 840.0).abs() < 1e-4);
    assert!((two_x.blend[0] - one_x.blend[0] * 2.0).abs() < 1e-4);
    assert!((two_x.horizon[0] - one_x.horizon[0] * 2.0).abs() < 1e-4);
    assert!((two_x.horizon[1] - one_x.horizon[1] * 2.0).abs() < 1e-4);

    let globe_sky = sky_metadata(&sky, &view, 0.5, 840.0);
    assert!((globe_sky.blend[1] - 0.5).abs() < 1e-6);
    assert!(globe_sky.inverse_projection[2][0] > 0.0);
    assert!((globe_sky.inverse_projection[2][1] - 2.0).abs() < 1e-4);
    assert!(globe_sky.inverse_projection[2][2] > 0.0);
    assert!(globe_sky.up_and_horizon[..3].iter().any(|&v| v != 0.0));
}

#[test]
fn terrain_fog_transitions_smoothly_across_globe_zoom_levels() {
    use cgmath::{Deg, Rad};

    use crate::{
        coords::{WorldCoords, Zoom},
        render::view_state::ViewState,
        style::Style,
        terrain::queue_system::uniforms::terrain_fog,
        window::{LogicalSize, PhysicalSize},
    };

    let style: Style = serde_json::from_str(
        r##"{
            "version": 8,
            "sources": {},
            "layers": [],
            "projection": { "type": "globe" },
            "sky": {
                "sky-color": "#88C6FC",
                "horizon-color": "#ffffff",
                "fog-color": "#ffffff",
                "fog-ground-blend": 0.5,
                "horizon-fog-blend": 0.8,
                "sky-horizon-blend": 0.8,
                "atmosphere-blend": ["interpolate", ["linear"], ["zoom"], 0, 1, 5, 1, 7, 0]
            }
        }"##,
    )
    .expect("style parses");

    let fog_at = |zoom: f64| {
        let mut view = ViewState::new(
            PhysicalSize::new(800, 420).expect("physical size"),
            WorldCoords::from_lat_lon(LatLon::new(47.27574, 11.39085), Zoom::new(zoom)),
            Zoom::new(zoom),
            Deg(0.0),
            Rad(0.6435011087932844),
        );
        view.set_max_pitch(Deg(85.0));
        view.camera_mut().set_pitch(Deg(70.0));
        view.resize(LogicalSize::new(800, 420).expect("logical size"));
        terrain_fog(&style, &view)
    };

    let z12 = fog_at(12.0);
    let z11_99 = fog_at(11.99);
    let z11_5 = fog_at(11.5);
    let z11 = fog_at(11.0);
    let z6 = fog_at(6.0);
    let z4 = fog_at(4.0);

    assert!(!z12.globe && (z12.opacity - 1.0).abs() < 1e-5);
    assert!(!z11_99.globe && (z11_99.opacity - 1.0).abs() < 1e-5);
    assert!(!z11_5.globe && (z11_5.opacity - 1.0).abs() < 1e-5);
    assert!(!z11.globe && (z11.opacity - 1.0).abs() < 1e-5);
    assert!(!z6.globe && z6.opacity > 0.3 && z6.opacity < 0.7);
    assert!(z4.globe && z4.opacity == 0.0);
}
