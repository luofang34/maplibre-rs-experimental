//! Extruded polygon shader: the vector buffer layout with the extrusion slots exposed.

use super::{
    attribute, FillShaderFeatureMetadata, GpuShaderVertex, Shader, ShaderLayerMetadata,
    ShaderTileMetadata,
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

fn extrusion_vertex_source() -> &'static str {
    concat!(
        include_str!(concat!(
            env!("OUT_DIR"),
            "/wgsl/render/shaders/projection.vertex.wgsl"
        )),
        include_str!(concat!(
            env!("OUT_DIR"),
            "/wgsl/render/shaders/fill_extrusion.vertex.wgsl"
        ))
    )
}

fn extrusion_layer_layout(include_uniform_color: bool) -> VertexBufferLayout {
    let mut attributes = vec![
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
    ];
    if include_uniform_color {
        attributes.push(attribute(
            std::mem::offset_of!(ShaderLayerMetadata, uniform_color) as u64,
            wgpu::VertexFormat::Float32x4,
            8,
        ));
    }
    VertexBufferLayout {
        array_stride: 0,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes,
    }
}

/// Compact 8-byte vertex layout for uniform-height/base extruded polygons (`FillExtrusionCompactShader`).
pub struct FillExtrusionCompactShader {
    /// Format of the color attachment used by the render pipeline.
    pub format: wgpu::TextureFormat,
    /// Which pass of an extrusion the pipeline draws.
    pub pass: ExtrusionPass,
}

impl Shader for FillExtrusionShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: extrusion_vertex_source(),
            entry_point: match self.pass {
                ExtrusionPass::TranslucentBack => "translucent_back_main",
                ExtrusionPass::TranslucentFront => "translucent_front_main",
                _ => "main",
            },
            buffers: vec![
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuShaderVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: wgpu::vertex_attr_array![
                        0 => Sint16x2,
                        1 => Uint32,
                        10 => Uint32x2
                    ]
                    .to_vec(),
                },
                ShaderTileMetadata::basic_layout(),
                extrusion_layer_layout(false),
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<FillShaderFeatureMetadata>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: vec![attribute(0, wgpu::VertexFormat::Unorm8x4, 8)],
                },
            ],
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/fill_extrusion.fragment.wgsl"
            )),
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

impl Shader for FillExtrusionCompactShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: extrusion_vertex_source(),
            entry_point: match self.pass {
                ExtrusionPass::TranslucentBack => "compact_translucent_back_main",
                ExtrusionPass::TranslucentFront => "compact_translucent_front_main",
                _ => "compact_main",
            },
            buffers: vec![
                VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: wgpu::vertex_attr_array![
                        0 => Sint16x2,
                        1 => Uint32
                    ]
                    .to_vec(),
                },
                ShaderTileMetadata::basic_layout(),
                extrusion_layer_layout(true),
            ],
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FillExtrusionShader {
            format: self.format,
            pass: self.pass,
        }
        .describe_fragment()
    }
}
