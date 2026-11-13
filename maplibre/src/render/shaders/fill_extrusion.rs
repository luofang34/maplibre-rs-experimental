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
    /// The colour pass with an image in place of the colour.
    PatternColor,
    /// Nothing but the stencil reset of the pixels the colour pass marked.
    Clear,
    /// Back-facing shell of a translucent 3D volume, depth-tested without writing depth.
    TranslucentBack,
    /// Front-facing shell of a translucent 3D volume, depth-tested without writing depth.
    TranslucentFront,
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
        VertexState {
            source: concat!(
                include_str!("projection.vertex.wgsl"),
                include_str!("fill_extrusion.vertex.wgsl")
            ),
            entry_point: match self.pass {
                ExtrusionPass::TranslucentBack => "translucent_back_main",
                ExtrusionPass::TranslucentFront => "translucent_front_main",
                _ => "main",
            },
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
                        // The distance along the wall and, from `corner_code` after it, the
                        // height one zoom up packed with the tile's overscale.
                        attribute(
                            std::mem::offset_of!(ShaderVertex, edge_distance) as u64,
                            wgpu::VertexFormat::Float32x2,
                            10,
                        ),
                    ],
                },
                ShaderTileMetadata::basic_layout(),
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
            entry_point: match self.pass {
                ExtrusionPass::PatternColor => "pattern_main",
                _ => "main",
            },
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: match self.pass {
                    ExtrusionPass::Depth | ExtrusionPass::Clear => wgpu::ColorWrites::empty(),
                    ExtrusionPass::Color
                    | ExtrusionPass::PatternColor
                    | ExtrusionPass::TranslucentBack
                    | ExtrusionPass::TranslucentFront => wgpu::ColorWrites::ALL,
                },
            })],
        }
    }
}
