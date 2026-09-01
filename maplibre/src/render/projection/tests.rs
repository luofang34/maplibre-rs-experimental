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

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn projection_aware_tile_pipelines_compile() {
    use crate::render::{
        resource::{RenderPipeline, TilePipeline},
        settings::RendererSettings,
        shaders::{FillShader, LineShader, RasterShader, Shader, TileMaskShader},
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
    let shaders: [(&str, Box<dyn Shader>, bool); 4] = [
        ("test fill", Box::new(FillShader { format }), false),
        ("test line", Box::new(LineShader { format }), false),
        (
            "test mask",
            Box::new(TileMaskShader {
                format,
                draw_colors: false,
                debug_lines: false,
            }),
            false,
        ),
        ("test raster", Box::new(RasterShader { format }), true),
    ];

    for (name, shader, raster) in shaders {
        TilePipeline::new(
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
            false,
        )
        .describe_render_pipeline()
        .initialize_with_prefix_layouts(&device, &[projection.bind_group_layout()]);
    }
}
