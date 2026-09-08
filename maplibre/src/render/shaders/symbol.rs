//! Symbol pipeline vertex buffers and premultiplied color blending.
use super::*;

pub struct SymbolShader {
    pub format: wgpu::TextureFormat,
}

impl Shader for SymbolShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: concat!(
                include_str!("projection.vertex.wgsl"),
                include_str!("symbol_uniforms.wgsl"),
                include_str!("sdf_new.vertex.wgsl")
            ),
            entry_point: "main",
            buffers: vec![
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<ShaderSymbolVertexNew>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes:
                        wgpu::vertex_attr_array![0 => Sint32x4, 1 => Uint32x4, 2 => Sint32x4]
                            .to_vec(),
                },
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<ShaderTileMetadata>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes:
                        wgpu::vertex_attr_array![4 => Float32x4, 5 => Float32x4, 6 => Float32x4,
                        7 => Float32x4, 9 => Float32, 8 => Float32, 11 => Float32, 3 => Float32x4]
                        .to_vec(),
                },
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<ShaderLayerMetadata>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: wgpu::vertex_attr_array![10 => Float32, 13 => Float32].to_vec(),
                },
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<SDFShaderFeatureMetadata>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: wgpu::vertex_attr_array![12 => Float32x2].to_vec(),
                },
            ],
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: concat!(
                include_str!("symbol_uniforms.wgsl"),
                include_str!("sdf_new.fragment.wgsl")
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
