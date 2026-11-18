//! Circle shader descriptor: the vector buffer layout with the circle slots of the layer
//! metadata exposed.

use super::{
    f32_to_f16_bits, FillShaderFeatureMetadata, GpuShaderVertex, Shader, ShaderLayerMetadata,
    ShaderTileMetadata,
};
use crate::render::resource::{FragmentState, VertexBufferLayout, VertexState};

/// Packs `[radius, radius_next]` in screen pixels into the second 4-byte `FillShaderFeatureMetadata`
/// slot of [`CircleDynamicRadiusShader`].
///
/// Uses `0x8000_0000` (`(0.0, -0.0)` in IEEE-754 binary16) when both radii round to zero so
/// `dyn_radius_packed != 0u` still distinguishes a dynamic zero radius from static `CircleShader`.
#[inline]
pub fn pack_dynamic_circle_radii(radius: f32, radius_next: f32) -> FillShaderFeatureMetadata {
    let r0 = u32::from(f32_to_f16_bits(radius.max(0.0)));
    let r1 = u32::from(f32_to_f16_bits(radius_next.max(0.0)));
    let packed = r0 | (r1 << 16);
    let non_zero = if packed == 0 { 0x8000_0000 } else { packed };
    FillShaderFeatureMetadata {
        color: non_zero.to_le_bytes(),
    }
}

/// The circle layer shader pair; the fragment output matches `format`.
pub struct CircleShader {
    /// Colour format of the render target the circles draw into.
    pub format: wgpu::TextureFormat,
}

impl Shader for CircleShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: concat!(
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/render/shaders/projection.vertex.wgsl"
                )),
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/render/shaders/circle.vertex.wgsl"
                ))
            ),
            entry_point: "main",
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
                {
                    let mut tile_attrs = ShaderTileMetadata::transform_attributes().to_vec();
                    tile_attrs.extend([
                        super::attribute(
                            std::mem::offset_of!(ShaderTileMetadata, zoom_factor) as u64,
                            wgpu::VertexFormat::Float32,
                            9,
                        ),
                        super::attribute(
                            std::mem::offset_of!(ShaderTileMetadata, viewport_width) as u64,
                            wgpu::VertexFormat::Float32,
                            11,
                        ),
                        super::attribute(
                            std::mem::offset_of!(ShaderTileMetadata, viewport_height) as u64,
                            wgpu::VertexFormat::Float32,
                            12,
                        ),
                        super::attribute(
                            std::mem::offset_of!(ShaderTileMetadata, tile_mercator_coords) as u64,
                            wgpu::VertexFormat::Float32x4,
                            2,
                        ),
                    ]);
                    ShaderTileMetadata::instance_layout(tile_attrs)
                },
                circle_layer_layout(true, true),
                VertexBufferLayout {
                    array_stride: std::mem::size_of::<FillShaderFeatureMetadata>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: wgpu::vertex_attr_array![8 => Unorm8x4].to_vec(),
                },
            ],
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/render/shaders/circle.fragment.wgsl"
            )),
            entry_point: "main",
            targets: vec![Some(wgpu::ColorTargetState {
                format: self.format,
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
}

/// Circle shader variant whose Slot 3 carries both per-vertex fill (`@location(8)`) and
/// per-vertex stroke (`@location(3)`) colours (`8 B` stride).
pub struct CircleFeatureStrokeShader {
    /// Colour format of the render target the circles draw into.
    pub format: wgpu::TextureFormat,
}

impl Shader for CircleFeatureStrokeShader {
    fn describe_vertex(&self) -> VertexState {
        let mut base = CircleShader {
            format: self.format,
        }
        .describe_vertex();
        base.buffers[2] = circle_layer_layout(false, true);
        base.buffers[3] = VertexBufferLayout {
            array_stride: 2 * std::mem::size_of::<FillShaderFeatureMetadata>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: wgpu::vertex_attr_array![8 => Unorm8x4, 3 => Unorm8x4].to_vec(),
        };
        base
    }

    fn describe_fragment(&self) -> FragmentState {
        CircleShader {
            format: self.format,
        }
        .describe_fragment()
    }
}

/// Circle shader variant whose Slot 3 carries per-vertex fill (`@location(8)`) and packed
/// `[radius_f16, radius_next_f16]` (`@location(15)`, `8 B` stride) for in-place GeoJSON updates.
pub struct CircleDynamicRadiusShader {
    /// Colour format of the render target the circles draw into.
    pub format: wgpu::TextureFormat,
}

impl Shader for CircleDynamicRadiusShader {
    fn describe_vertex(&self) -> VertexState {
        let mut base = CircleShader {
            format: self.format,
        }
        .describe_vertex();
        base.buffers[2] = circle_layer_layout(true, false);
        base.buffers[3] = VertexBufferLayout {
            array_stride: 2 * std::mem::size_of::<FillShaderFeatureMetadata>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: wgpu::vertex_attr_array![8 => Unorm8x4, 15 => Uint32].to_vec(),
        };
        base
    }

    fn describe_fragment(&self) -> FragmentState {
        CircleShader {
            format: self.format,
        }
        .describe_fragment()
    }
}

fn circle_layer_layout(include_stroke: bool, include_zero_dyn_radius: bool) -> VertexBufferLayout {
    let mut attributes = Vec::with_capacity(4);
    if include_stroke {
        attributes.push(wgpu::VertexAttribute {
            offset: std::mem::offset_of!(ShaderLayerMetadata, stroke_color) as u64,
            format: wgpu::VertexFormat::Float32x4,
            shader_location: 3,
        });
    }
    if include_zero_dyn_radius {
        attributes.push(wgpu::VertexAttribute {
            offset: (std::mem::offset_of!(ShaderLayerMetadata, circle_params) + 12) as u64,
            format: wgpu::VertexFormat::Uint32,
            shader_location: 15,
        });
    }
    attributes.extend([
        wgpu::VertexAttribute {
            offset: std::mem::offset_of!(ShaderLayerMetadata, circle_params) as u64,
            format: wgpu::VertexFormat::Float32x4,
            shader_location: 13,
        },
        wgpu::VertexAttribute {
            offset: std::mem::offset_of!(ShaderLayerMetadata, circle_flags) as u64,
            format: wgpu::VertexFormat::Float32x4,
            shader_location: 14,
        },
    ]);
    VertexBufferLayout {
        array_stride: 0,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes,
    }
}
