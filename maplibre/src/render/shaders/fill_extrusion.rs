//! Extruded polygon shader: the vector buffer layout with the extrusion slots exposed.

use super::{
    attribute, FillShaderFeatureMetadata, Shader, ShaderLayerMetadata, ShaderTileMetadata,
    ShaderVertex,
};
use crate::render::resource::{FragmentState, VertexBufferLayout, VertexState};

/// Which pass of an extrusion a pipeline draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtrusionPass {
    /// Depth only, so the colour pass shades just the nearest surface of the layer.
    Depth,
    /// Lit colour where the depth pass left its nearest surface, once per pixel.
    Color,
    /// Nothing but the stencil reset of the pixels the colour pass marked.
    Clear,
}

/// Walls and roofs lit by the root light, with premultiplied-alpha output.
pub struct FillExtrusionShader {
    /// Format of the color attachment used by the render pipeline.
    pub format: wgpu::TextureFormat,
    /// Whether colour is written.
    pub pass: ExtrusionPass,
}

impl Shader for FillExtrusionShader {
    fn describe_vertex(&self) -> VertexState {
        let vec4 = wgpu::VertexFormat::Float32x4.size();
        VertexState {
            source: concat!(
                include_str!("projection.vertex.wgsl"),
                include_str!("fill_extrusion.vertex.wgsl")
            ),
            entry_point: "main",
            buffers: vec![
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<ShaderVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: vec![
                        attribute(0, wgpu::VertexFormat::Float32x2, 0),
                        attribute(
                            wgpu::VertexFormat::Float32x2.size(),
                            wgpu::VertexFormat::Float32x2,
                            1,
                        ),
                        attribute(
                            std::mem::offset_of!(ShaderVertex, distance) as u64,
                            wgpu::VertexFormat::Float32,
                            3,
                        ),
                        attribute(
                            std::mem::offset_of!(ShaderVertex, elevation) as u64,
                            wgpu::VertexFormat::Float32,
                            11,
                        ),
                    ],
                },
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<ShaderTileMetadata>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: vec![
                        attribute(0, wgpu::VertexFormat::Float32x4, 4),
                        attribute(vec4, wgpu::VertexFormat::Float32x4, 5),
                        attribute(2 * vec4, wgpu::VertexFormat::Float32x4, 6),
                        attribute(3 * vec4, wgpu::VertexFormat::Float32x4, 7),
                        attribute(4 * vec4, wgpu::VertexFormat::Float32, 9),
                        attribute(
                            4 * vec4 + 3 * wgpu::VertexFormat::Float32.size(),
                            wgpu::VertexFormat::Float32x4,
                            2,
                        ),
                    ],
                },
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<ShaderLayerMetadata>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: vec![
                        attribute(
                            std::mem::offset_of!(ShaderLayerMetadata, translate) as u64,
                            wgpu::VertexFormat::Float32x2,
                            15,
                        ),
                        attribute(
                            std::mem::offset_of!(ShaderLayerMetadata, stroke_color) as u64,
                            wgpu::VertexFormat::Float32x4,
                            12,
                        ),
                        attribute(
                            std::mem::offset_of!(ShaderLayerMetadata, circle_params) as u64,
                            wgpu::VertexFormat::Float32x4,
                            13,
                        ),
                        attribute(
                            std::mem::offset_of!(ShaderLayerMetadata, circle_flags) as u64,
                            wgpu::VertexFormat::Float32x4,
                            14,
                        ),
                    ],
                },
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<FillShaderFeatureMetadata>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: vec![attribute(0, wgpu::VertexFormat::Float32x4, 8)],
                },
            ],
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!("fill_extrusion.fragment.wgsl"),
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: match self.pass {
                    ExtrusionPass::Depth | ExtrusionPass::Clear => wgpu::ColorWrites::empty(),
                    ExtrusionPass::Color => wgpu::ColorWrites::ALL,
                },
            })],
        }
    }
}
