use super::Shader;
use crate::render::resource::{FragmentState, VertexBufferLayout, VertexState};

/// Vertex of the terrain grid in tile units with a skirt marker.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, bytemuck_derive::Pod, bytemuck_derive::Zeroable)]
pub struct TerrainVertex {
    /// Horizontal tile coordinate.
    pub x: i16,
    /// Vertical tile coordinate.
    pub y: i16,
    /// One for skirt vertices, which the shader drops by the skirt length.
    pub skirt: u16,
    padding: u16,
}

impl TerrainVertex {
    /// Constructs a terrain grid or skirt vertex from integer tile coordinates.
    pub const fn new(x: i32, y: i32, skirt: u16) -> Self {
        Self {
            x: x as i16,
            y: y as i16,
            skirt,
            padding: 0,
        }
    }
}

/// Terrain mesh shader: elevation from a DEM texture, color from a drape texture.
pub struct TerrainShader {
    /// Format of the color attachment receiving the draped terrain.
    pub format: wgpu::TextureFormat,
}

impl Shader for TerrainShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: concat!(
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/engine/render/gpu/shaders/projection.vertex.wgsl"
                )),
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/engine/render/gpu/shaders/terrain_surface.wgsl.part"
                )),
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/engine/render/gpu/shaders/terrain.vertex.wgsl"
                ))
            ),
            entry_point: "main",
            buffers: vec![VertexBufferLayout {
                array_stride: std::mem::size_of::<TerrainVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: vec![
                    wgpu::VertexAttribute {
                        offset: 0,
                        format: wgpu::VertexFormat::Sint16x2,
                        shader_location: 0,
                    },
                    wgpu::VertexAttribute {
                        offset: wgpu::VertexFormat::Sint16x2.size(),
                        format: wgpu::VertexFormat::Uint16x2,
                        shader_location: 1,
                    },
                ],
            }],
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: concat!(
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/engine/render/gpu/shaders/daylight.wgsl.part"
                )),
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/engine/render/gpu/shaders/terrain.fragment.wgsl"
                ))
            ),
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
                // The drape holds premultiplied colours, so blending them over what is drawn
                // multiplies by alpha once; a translucent drape shows the terrain behind it,
                // its skirts included, as GL JS draws terrain in its translucent pass.
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }
    }
}
