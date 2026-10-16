use super::{
    attribute, FillShaderFeatureMetadata, Shader, ShaderLayerMetadata, ShaderTileMetadata,
    ShaderVertex,
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
            include_str!("projection.vertex.wgsl"),
            include_str!("line_ground.wgsl"),
            include_str!("line_flat.wgsl"),
            include_str!("line.vertex.wgsl")
        ))
    }

    fn describe_fragment(&self) -> FragmentState {
        line_fragment(self.format)
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
            include_str!("projection.vertex.wgsl"),
            include_str!("line_ground.wgsl"),
            include_str!("terrain_surface.wgsl.part"),
            include_str!("line_terrain.wgsl"),
            include_str!("line.vertex.wgsl")
        ))
    }

    fn describe_fragment(&self) -> FragmentState {
        line_fragment(self.format)
    }
}

fn line_vertex(source: &'static str) -> VertexState {
    VertexState {
        source,
        entry_point: "main",
        buffers: vec![
            // vertex data
            VertexBufferLayout {
                array_stride: std::mem::size_of::<ShaderVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: vec![
                    attribute(0, wgpu::VertexFormat::Float32x2, 0),
                    attribute(
                        wgpu::VertexFormat::Float32x2.size(),
                        wgpu::VertexFormat::Float32x4,
                        1,
                    ),
                    // Packed per-feature width and offset; zero bits leave the layer's own.
                    attribute(
                        std::mem::offset_of!(ShaderVertex, edge_distance) as u64,
                        wgpu::VertexFormat::Float32,
                        10,
                    ),
                ],
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
                attributes: vec![attribute(0, wgpu::VertexFormat::Float32x4, 8)],
            },
        ],
    }
}

fn line_fragment(format: wgpu::TextureFormat) -> FragmentState {
    FragmentState {
        source: include_str!("line.fragment.wgsl"),
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
    VertexBufferLayout {
        array_stride: std::mem::size_of::<ShaderTileMetadata>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: vec![
            attribute(0, wgpu::VertexFormat::Float32x4, 4),
            attribute(
                wgpu::VertexFormat::Float32x4.size(),
                wgpu::VertexFormat::Float32x4,
                5,
            ),
            attribute(
                2 * wgpu::VertexFormat::Float32x4.size(),
                wgpu::VertexFormat::Float32x4,
                6,
            ),
            attribute(
                3 * wgpu::VertexFormat::Float32x4.size(),
                wgpu::VertexFormat::Float32x4,
                7,
            ),
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
                4 * wgpu::VertexFormat::Float32x4.size() + 3 * wgpu::VertexFormat::Float32.size(),
                wgpu::VertexFormat::Float32x4,
                2,
            ),
            attribute(
                5 * wgpu::VertexFormat::Float32x4.size() + 3 * wgpu::VertexFormat::Float32.size(),
                wgpu::VertexFormat::Uint32,
                14,
            ),
        ],
    }
}
