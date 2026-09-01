use std::mem::{align_of, size_of};

use cgmath::{Matrix4, Vector4};

use super::ShaderProjectionData;
use crate::{
    projection::renderer_data::RendererProjectionData,
    render::shaders::ShaderTileMetadata,
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
