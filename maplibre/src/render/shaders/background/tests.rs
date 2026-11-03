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
