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
    /// Back-facing shell of a translucent 3D volume, depth-tested without writing depth.
    TranslucentBack,
    /// Front-facing shell of a translucent 3D volume, depth-tested without writing depth.
    TranslucentFront,
}

/// Fullscreen 3-vertex triangle pass clearing bit 7 (`STENCIL_MARK`) without re-transforming tile geometry.
pub struct StencilClearShader {
    /// Format of the color attachment used by the render pipeline.
    pub format: wgpu::TextureFormat,
}

impl Shader for StencilClearShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/engine/render/gpu/shaders/stencil_clear.vertex.wgsl"
            )),
            entry_point: "main",
            buffers: Vec::new(),
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/engine/render/gpu/shaders/fill_extrusion.fragment.wgsl"
            )),
            entry_point: "depth_flat",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::empty(),
            })],
        }
    }
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
            "/wgsl/engine/render/gpu/shaders/projection.vertex.wgsl"
        )),
        include_str!(concat!(
            env!("OUT_DIR"),
            "/wgsl/engine/render/gpu/shaders/fill_extrusion.vertex.wgsl"
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
        array_stride: std::mem::size_of::<ShaderLayerMetadata>() as u64,
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

fn extrusion_fragment_entry(pass: ExtrusionPass, globe: bool) -> &'static str {
    match pass {
        ExtrusionPass::PatternColor => "pattern_main",
        ExtrusionPass::Depth => {
            if globe {
                "depth_globe"
            } else {
                "depth_flat"
            }
        }
        ExtrusionPass::Color | ExtrusionPass::TranslucentBack | ExtrusionPass::TranslucentFront => {
            if globe {
                "main"
            } else {
                "main_flat"
            }
        }
    }
}

fn extrusion_vertex_layout() -> VertexBufferLayout {
    VertexBufferLayout {
        array_stride: std::mem::size_of::<GpuShaderVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: wgpu::vertex_attr_array![
            0 => Sint16x2,
            1 => Uint32,
            10 => Uint32x2
        ]
        .to_vec(),
    }
}

fn extrusion_feature_layout() -> VertexBufferLayout {
    VertexBufferLayout {
        array_stride: std::mem::size_of::<FillShaderFeatureMetadata>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: vec![attribute(0, wgpu::VertexFormat::Unorm8x4, 8)],
    }
}

fn extrusion_compact_vertex_layout() -> VertexBufferLayout {
    VertexBufferLayout {
        array_stride: 8,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: wgpu::vertex_attr_array![
            0 => Sint16x2,
            1 => Uint32
        ]
        .to_vec(),
    }
}

impl FillExtrusionShader {
    /// Selects either the flat zero-globe vertex entry points or the globe-aware variants.
    pub fn describe_vertex_for_globe(&self, globe: bool) -> VertexState {
        let mut vertex = self.describe_vertex();
        if !globe {
            vertex.entry_point = match self.pass {
                ExtrusionPass::Depth => "flat_depth_main",
                ExtrusionPass::Color => "flat_main",
                ExtrusionPass::PatternColor => "pattern_main",
                ExtrusionPass::TranslucentBack => "translucent_back_main",
                ExtrusionPass::TranslucentFront => "translucent_front_main",
            };
        }
        vertex
    }

    /// Selects either the flat zero-`discard` fragment entry points or the globe-clipped variants.
    pub fn describe_fragment_for_globe(&self, globe: bool) -> FragmentState {
        FragmentState {
            source: include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/engine/render/gpu/shaders/fill_extrusion.fragment.wgsl"
            )),
            entry_point: extrusion_fragment_entry(self.pass, globe),
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: match self.pass {
                    ExtrusionPass::Depth => wgpu::ColorWrites::empty(),
                    ExtrusionPass::Color
                    | ExtrusionPass::PatternColor
                    | ExtrusionPass::TranslucentBack
                    | ExtrusionPass::TranslucentFront => wgpu::ColorWrites::ALL,
                },
            })],
        }
    }
}

impl Shader for FillExtrusionShader {
    fn describe_vertex(&self) -> VertexState {
        let mut buffers = vec![
            extrusion_vertex_layout(),
            ShaderTileMetadata::basic_layout(),
            extrusion_layer_layout(false),
        ];
        if self.pass != ExtrusionPass::Depth {
            buffers.push(extrusion_feature_layout());
        }
        VertexState {
            source: extrusion_vertex_source(),
            entry_point: match self.pass {
                ExtrusionPass::Depth => "depth_main",
                ExtrusionPass::PatternColor => "pattern_main",
                ExtrusionPass::TranslucentBack => "translucent_back_main",
                ExtrusionPass::TranslucentFront => "translucent_front_main",
                ExtrusionPass::Color => "main",
            },
            buffers,
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        self.describe_fragment_for_globe(true)
    }
}

impl FillExtrusionCompactShader {
    /// Selects either the flat zero-globe vertex entry points or the globe-aware variants.
    pub fn describe_vertex_for_globe(&self, globe: bool) -> VertexState {
        let mut vertex = self.describe_vertex();
        if !globe {
            vertex.entry_point = match self.pass {
                ExtrusionPass::Depth => "flat_compact_depth_main",
                ExtrusionPass::Color | ExtrusionPass::PatternColor => "flat_compact_main",
                ExtrusionPass::TranslucentBack => "compact_translucent_back_main",
                ExtrusionPass::TranslucentFront => "compact_translucent_front_main",
            };
        }
        vertex
    }

    /// Selects either the flat zero-`discard` fragment entry points or the globe-clipped variants.
    pub fn describe_fragment_for_globe(&self, globe: bool) -> FragmentState {
        FillExtrusionShader {
            format: self.format,
            pass: self.pass,
        }
        .describe_fragment_for_globe(globe)
    }
}

impl Shader for FillExtrusionCompactShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: extrusion_vertex_source(),
            entry_point: match self.pass {
                ExtrusionPass::Depth => "compact_depth_main",
                ExtrusionPass::TranslucentBack => "compact_translucent_back_main",
                ExtrusionPass::TranslucentFront => "compact_translucent_front_main",
                ExtrusionPass::Color | ExtrusionPass::PatternColor => "compact_main",
            },
            buffers: vec![
                extrusion_compact_vertex_layout(),
                ShaderTileMetadata::basic_layout(),
                extrusion_layer_layout(true),
            ],
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        self.describe_fragment_for_globe(true)
    }
}
