//! Symbol pipeline vertex buffers and premultiplied color blending.
use super::*;

/// Text and icon shader with atlas sampling, collision opacity and terrain occlusion.
pub struct SymbolShader {
    /// Format of the color attachment used by the render pipeline.
    pub format: wgpu::TextureFormat,
}

impl Shader for SymbolShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: concat!(
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/render/shaders/projection.vertex.wgsl"
                )),
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/render/shaders/symbol_uniforms.wgsl"
                )),
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/render/shaders/sdf.vertex.wgsl"
                ))
            ),
            entry_point: "main",
            buffers: vec![
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<ShaderSymbolVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes:
                        wgpu::vertex_attr_array![0 => Sint16x4, 1 => Uint16x4, 2 => Sint32x2]
                            .to_vec(),
                },
                {
                    let mut tile_attrs = ShaderTileMetadata::transform_attributes().to_vec();
                    tile_attrs.extend([
                        attribute(
                            std::mem::offset_of!(ShaderTileMetadata, zoom_factor) as u64,
                            wgpu::VertexFormat::Float32,
                            9,
                        ),
                        attribute(
                            std::mem::offset_of!(ShaderTileMetadata, viewport_width) as u64,
                            wgpu::VertexFormat::Float32,
                            8,
                        ),
                        attribute(
                            std::mem::offset_of!(ShaderTileMetadata, viewport_height) as u64,
                            wgpu::VertexFormat::Float32,
                            11,
                        ),
                        attribute(
                            std::mem::offset_of!(ShaderTileMetadata, tile_mercator_coords) as u64,
                            wgpu::VertexFormat::Float32x4,
                            3,
                        ),
                    ]);
                    ShaderTileMetadata::instance_layout(tile_attrs)
                },
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<SDFShaderFeatureMetadata>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: wgpu::vertex_attr_array![
                        15 => Uint32x4, 12 => Float32x2, 10 => Unorm8x4, 13 => Unorm8x4
                    ]
                    .to_vec(),
                },
            ],
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: concat!(
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/render/shaders/symbol_uniforms.wgsl"
                )),
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/render/shaders/sdf.fragment.wgsl"
                ))
            ),
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                write_mask: wgpu::ColorWrites::ALL,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
            })],
        }
    }
}
