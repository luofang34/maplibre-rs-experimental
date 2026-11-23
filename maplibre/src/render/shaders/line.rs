use super::{
    attribute, FillShaderFeatureMetadata, GpuShaderVertex, Shader, ShaderLayerMetadata,
    ShaderTileMetadata,
};
use crate::render::resource::{FragmentState, VertexBufferLayout, VertexState};

/// Stroked-path shader with screen-pixel width and optional terrain-relative elevation.
pub struct LineShader {
    /// Format of the color attachment used by the render pipeline.
    pub format: wgpu::TextureFormat,
}

impl Shader for LineShader {
    fn describe_vertex(&self) -> VertexState {
        line_vertex(concat!(
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/projection.vertex.wgsl"
            )),
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/line_ground.wgsl"
            )),
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/line_flat.wgsl"
            )),
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/line.vertex.wgsl"
            ))
        ))
    }

    fn describe_fragment(&self) -> FragmentState {
        line_fragment(
            self.format,
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/line.fragment.wgsl"
            )),
        )
    }
}

/// Lean stroked-path shader for plain solid lines without dashes, patterns, gradients, or terrain.
pub struct PlainLineShader {
    /// Format of the color attachment used by the render pipeline.
    pub format: wgpu::TextureFormat,
    /// Uses globe projection when `true`, or pure Mercator projection when `false`.
    pub globe: bool,
}

impl Shader for PlainLineShader {
    fn describe_vertex(&self) -> VertexState {
        let source = if self.globe {
            concat!(
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/render/shaders/projection.vertex.wgsl"
                )),
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/render/shaders/line_plain_globe.vertex.wgsl"
                ))
            )
        } else {
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/line_plain_flat.vertex.wgsl"
            ))
        };
        line_vertex(source)
    }

    fn describe_fragment(&self) -> FragmentState {
        line_fragment(
            self.format,
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/line_plain.fragment.wgsl"
            )),
        )
    }
}

/// Stroked-path shader for a line on the terrain surface: the terrain tile bound at the third
/// bind group gives each vertex its height, and the line keeps its width in screen pixels.
pub struct TerrainLineShader {
    /// Format of the color attachment used by the render pipeline.
    pub format: wgpu::TextureFormat,
}

impl Shader for TerrainLineShader {
    fn describe_vertex(&self) -> VertexState {
        line_vertex(concat!(
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/projection.vertex.wgsl"
            )),
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/line_ground.wgsl"
            )),
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/terrain_surface.wgsl.part"
            )),
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/line_terrain.wgsl"
            )),
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/line.vertex.wgsl"
            ))
        ))
    }

    fn describe_fragment(&self) -> FragmentState {
        line_fragment(
            self.format,
            include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/line.fragment.wgsl"
            )),
        )
    }
}

fn line_vertex(source: &'static str) -> VertexState {
    VertexState {
        source,
        entry_point: "main",
        buffers: vec![
            // vertex data
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
            tile_layout(),
            // layer metadata
            VertexBufferLayout {
                array_stride: std::mem::size_of::<ShaderLayerMetadata>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: vec![
                    attribute(
                        wgpu::VertexFormat::Float32.size(),
                        wgpu::VertexFormat::Float32,
                        13,
                    ),
                    attribute(
                        2 * wgpu::VertexFormat::Float32.size(),
                        wgpu::VertexFormat::Float32x2,
                        15,
                    ),
                    // Offset, gap width and blur of the line, in pixels.
                    attribute(
                        std::mem::offset_of!(ShaderLayerMetadata, circle_params) as u64,
                        wgpu::VertexFormat::Float32x4,
                        3,
                    ),
                ],
            },
            // features
            VertexBufferLayout {
                array_stride: std::mem::size_of::<FillShaderFeatureMetadata>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: vec![attribute(0, wgpu::VertexFormat::Unorm8x4, 8)],
            },
        ],
    }
}

fn line_fragment(format: wgpu::TextureFormat, source: &'static str) -> FragmentState {
    FragmentState {
        source,
        entry_point: "main",
        targets: vec![Some(wgpu::ColorTargetState {
            format,
            blend: Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            write_mask: wgpu::ColorWrites::ALL,
        })],
    }
}

fn tile_layout() -> VertexBufferLayout {
    let mut attrs = ShaderTileMetadata::transform_attributes().to_vec();
    attrs.extend([
        attribute(
            std::mem::offset_of!(ShaderTileMetadata, line_width_scale) as u64,
            wgpu::VertexFormat::Float32x2,
            9,
        ),
        // Viewport width and height, read together to leave a location for the clip region.
        attribute(
            std::mem::offset_of!(ShaderTileMetadata, viewport_width) as u64,
            wgpu::VertexFormat::Float32x2,
            11,
        ),
        attribute(
            std::mem::offset_of!(ShaderTileMetadata, clip_region) as u64,
            wgpu::VertexFormat::Float32x4,
            12,
        ),
        attribute(
            std::mem::offset_of!(ShaderTileMetadata, tile_mercator_coords) as u64,
            wgpu::VertexFormat::Float32x4,
            2,
        ),
        attribute(
            std::mem::offset_of!(ShaderTileMetadata, clip_antimeridian) as u64,
            wgpu::VertexFormat::Uint32,
            14,
        ),
    ]);
    ShaderTileMetadata::instance_layout(attrs)
}
