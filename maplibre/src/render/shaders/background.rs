//! Background and globe surface shader descriptors.

use bytemuck_derive::{Pod, Zeroable};

use super::{Shader, ShaderTileMetadata};
use crate::render::resource::{FragmentState, VertexBufferLayout, VertexState};

mod atmosphere;
pub use atmosphere::*;

#[cfg(test)]
mod tests;

/// Background color, painter index and viewport horizon supplied per draw.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct BackgroundLayerMetadata {
    /// Encoded-sRGB background color with straight alpha.
    pub color: [f32; 4],
    /// Style painter-order index carried by the instance layout.
    pub z_index: f32,
    /// Keeps the horizon sixteen-byte aligned.
    pub padding: [f32; 3],
    /// Horizon point in screen pixels with y up, then the unit normal pointing into the sky;
    /// the flat map ends there, as the tiles GL JS draws the background on do.
    pub horizon: [f32; 4],
    /// Viewport height in X, terrain-enabled flag in Y, then the frame's width and height in
    /// pixels.
    pub viewport: [f32; 4],
    /// Where the flat map ends north and south, relative to the view center: the row that
    /// turns a pixel into its numerator of the map row under it with the northern edge's map
    /// row behind it, then the denominator's row with the southern edge's.
    pub world_rows: [[f32; 4]; 2],
}

/// Fullscreen background clipped to the flat map's horizon.
pub struct BackgroundShader {
    /// Format of the color attachment receiving the background.
    pub format: wgpu::TextureFormat,
}

/// The fullscreen background with an image repeated over the map in place of the colour.
pub struct BackgroundPatternShader {
    /// Format of the color attachment receiving the background.
    pub format: wgpu::TextureFormat,
}

impl Shader for BackgroundPatternShader {
    fn describe_vertex(&self) -> VertexState {
        BackgroundShader {
            format: self.format,
        }
        .describe_vertex()
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/background_pattern.fragment.wgsl"
            )),
            ..BackgroundShader {
                format: self.format,
            }
            .describe_fragment()
        }
    }
}

/// Shader drawing background paint on the projected globe surface.
pub struct GlobeBackgroundShader {
    /// Render-target format.
    pub format: wgpu::TextureFormat,
}

impl GlobeBackgroundShader {
    /// Returns the opaque fragment state (`blend: None`) for globe backgrounds with alpha `1.0`.
    pub fn describe_opaque_fragment(&self) -> FragmentState {
        FragmentState {
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            ..self.describe_fragment()
        }
    }
}

/// Shader repeating an image over the projected globe surface in place of the colour.
pub struct GlobeBackgroundPatternShader {
    /// Render-target format.
    pub format: wgpu::TextureFormat,
}

impl Shader for GlobeBackgroundPatternShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: concat!(
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/render/shaders/projection.vertex.wgsl"
                )),
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/render/shaders/globe_background_pattern.vertex.wgsl"
                ))
            ),
            entry_point: "main",
            buffers: globe_background_buffers(),
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/globe_background_pattern.fragment.wgsl"
            )),
            ..GlobeBackgroundShader {
                format: self.format,
            }
            .describe_fragment()
        }
    }
}

impl Shader for GlobeBackgroundShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: concat!(
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/render/shaders/projection.vertex.wgsl"
                )),
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/render/shaders/globe_background.vertex.wgsl"
                ))
            ),
            entry_point: "main",
            buffers: globe_background_buffers(),
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/globe_background.fragment.wgsl"
            )),
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }
    }
}

fn globe_background_buffers() -> Vec<VertexBufferLayout> {
    let mut tile_attrs = ShaderTileMetadata::transform_attributes().to_vec();
    tile_attrs.push(wgpu::VertexAttribute {
        offset: std::mem::offset_of!(ShaderTileMetadata, tile_mercator_coords) as u64,
        format: wgpu::VertexFormat::Float32x4,
        shader_location: 2,
    });
    vec![
        crate::render::tile_mesh::GpuTileMeshVertex::vertex_buffer_layout(),
        ShaderTileMetadata::instance_layout(tile_attrs),
        VertexBufferLayout {
            array_stride: std::mem::size_of::<BackgroundLayerMetadata>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: vec![
                wgpu::VertexAttribute {
                    offset: 0,
                    format: wgpu::VertexFormat::Float32x4,
                    shader_location: 8,
                },
                wgpu::VertexAttribute {
                    offset: wgpu::VertexFormat::Float32x4.size(),
                    format: wgpu::VertexFormat::Float32,
                    shader_location: 10,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::offset_of!(BackgroundLayerMetadata, viewport) as u64,
                    format: wgpu::VertexFormat::Float32x4,
                    shader_location: 11,
                },
            ],
        },
    ]
}

impl BackgroundShader {
    /// Returns the zero-discard fragment state for unclipped flat backgrounds covering the viewport.
    pub fn describe_solid_fragment(&self) -> FragmentState {
        FragmentState {
            entry_point: "solid_main",
            ..self.describe_fragment()
        }
    }

    /// Returns the unclipped opaque (`blend: None`) fragment state for viewport-covering backgrounds.
    pub fn describe_solid_opaque_fragment(&self) -> FragmentState {
        FragmentState {
            entry_point: "solid_main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            ..self.describe_fragment()
        }
    }

    /// Returns the horizon-clipped opaque (`blend: None`) fragment state for solid flat backgrounds.
    pub fn describe_clipped_opaque_fragment(&self) -> FragmentState {
        FragmentState {
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            ..self.describe_fragment()
        }
    }
}

impl Shader for BackgroundShader {
    fn describe_vertex(&self) -> VertexState {
        let attr = |offset, format, shader_location| wgpu::VertexAttribute {
            offset,
            format,
            shader_location,
        };
        VertexState {
            source: include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/background.vertex.wgsl"
            )),
            entry_point: "main",
            buffers: vec![VertexBufferLayout {
                array_stride: std::mem::size_of::<BackgroundLayerMetadata>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: vec![
                    attr(0, wgpu::VertexFormat::Float32x4, 0),
                    attr(16, wgpu::VertexFormat::Float32, 1),
                    attr(32, wgpu::VertexFormat::Float32x4, 2),
                    attr(48, wgpu::VertexFormat::Float32x4, 3),
                    attr(64, wgpu::VertexFormat::Float32x4, 4),
                    attr(80, wgpu::VertexFormat::Float32x4, 5),
                ],
            }],
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/background.fragment.wgsl"
            )),
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }
    }
}
