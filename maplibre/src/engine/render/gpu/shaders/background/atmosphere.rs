//! Atmospheric scattering and sky dome shader descriptors.

use bytemuck_derive::{Pod, Zeroable};
use thiserror::Error;

use crate::{
    projection::{body::Body, globe::camera::GlobeCameraState},
    render::{
        resource::{FragmentState, VertexBufferLayout, VertexState},
        shaders::{Mat4x4f32, Shader},
        SceneLighting,
    },
    style::light::{LightError, LightSpecification},
};

fn resolve_sun_direction(
    camera: &GlobeCameraState,
    light: &LightSpecification,
    sky: SceneLighting,
    zoom: f64,
) -> Result<cgmath::Vector3<f64>, AtmosphereMetadataError> {
    match sky.sun_direction {
        Some(sun) => camera
            .world_direction_to_view(sun)
            .ok_or(LightError::InvalidViewDirection),
        None => light.sun_direction_in_view(camera, zoom),
    }
    .map_err(|source| AtmosphereMetadataError::Light { source })
}

/// Per-draw inputs for GL JS-compatible atmospheric scattering.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct AtmosphereLayerMetadata {
    pub(super) inverse_projection: Mat4x4f32,
    pub(super) globe_position: [f32; 4],
    pub(super) sun_direction: [f32; 4],
    pub(super) radius_blend_padding: [f32; 4],
}

impl AtmosphereLayerMetadata {
    /// Creates inert metadata for frames where no atmosphere draw is queued.
    pub fn disabled() -> Self {
        Self {
            inverse_projection: cgmath::Matrix4::from_scale(1.0).into(),
            globe_position: [0.0; 4],
            sun_direction: [0.0, 0.0, 1.0, 1.0],
            radius_blend_padding: [1.0, 0.0, Body::EARTH.radius_meters as f32, 0.0],
        }
    }

    /// The globe's centre in view axes, the sun's direction in view axes with the share of the
    /// sun an eclipse leaves as w, and the globe's radius in pixels, the blend and the body's
    /// radius in metres, as the scattering takes them.
    pub fn sky_inputs(&self) -> [[f32; 4]; 3] {
        [
            self.globe_position,
            self.sun_direction,
            self.radius_blend_padding,
        ]
    }

    /// Creates atmosphere inputs from the current globe camera and the host's sky: its sun, or
    /// the root light while the host has set none, and the share of the sun an eclipse leaves.
    pub fn from_view(
        camera: &GlobeCameraState,
        light: &LightSpecification,
        sky: SceneLighting,
        zoom: f64,
        blend: f32,
    ) -> Result<Self, AtmosphereMetadataError> {
        let inverse_projection = camera
            .inverse_projection()
            .cast::<f32>()
            .ok_or(AtmosphereMetadataError::FloatConversion)?;
        let globe_position = camera
            .globe_center_in_view()
            .cast::<f32>()
            .ok_or(AtmosphereMetadataError::FloatConversion)?;
        let sun_direction = resolve_sun_direction(camera, light, sky, zoom)?
            .cast::<f32>()
            .ok_or(AtmosphereMetadataError::FloatConversion)?;
        let radius = camera.globe_radius_pixels() as f32;
        if !radius.is_finite() || radius <= 0.0 || !blend.is_finite() {
            return Err(AtmosphereMetadataError::InvalidScalar { radius, blend });
        }
        Ok(Self {
            inverse_projection: inverse_projection.into(),
            globe_position: [globe_position.x, globe_position.y, globe_position.z, 0.0],
            sun_direction: [
                sun_direction.x,
                sun_direction.y,
                sun_direction.z,
                sky.sun_visible(),
            ],
            radius_blend_padding: [
                radius,
                blend.clamp(0.0, 1.0),
                camera.body().radius_meters as f32,
                0.0,
            ],
        })
    }

    /// Sets the reversed-Z clip depth (`1.0` over globe surface for orbital style atmosphere,
    /// `0.0` behind terrain/globe for low-altitude host atmosphere).
    pub fn with_depth_z(mut self, depth_z: f32) -> Self {
        self.radius_blend_padding[3] = depth_z.clamp(0.0, 1.0);
        self
    }
}

/// Invalid CPU inputs for the atmosphere shader.
#[derive(Clone, Copy, Debug, Error, PartialEq)]
pub enum AtmosphereMetadataError {
    /// The root light cannot be evaluated for this view.
    #[error("failed to evaluate atmosphere light")]
    Light {
        /// Underlying light error.
        #[source]
        source: LightError,
    },
    /// Camera values cannot be represented by the shader's f32 interface.
    #[error("atmosphere camera data cannot be represented as f32")]
    FloatConversion,
    /// Radius and blend must be finite and radius must be positive.
    #[error("invalid atmosphere radius {radius} or blend {blend}")]
    InvalidScalar {
        /// Globe radius in view-space pixels.
        radius: f32,
        /// Evaluated atmosphere blend.
        blend: f32,
    },
}

/// Per-frame values of the sky draw: colours, the horizon line on screen and the blends.
///
/// A negative `blend[2]` selects the globe sky, shaded by each pixel's elevation angle above
/// the eye's sea-level horizon instead of its distance from a screen-space horizon line.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct SkyLayerMetadata {
    /// Premultiplied sky colour.
    pub sky_color: [f32; 4],
    /// Premultiplied horizon colour.
    pub horizon_color: [f32; 4],
    /// Horizon point in screen pixels with y up, then the unit normal pointing into the sky.
    pub horizon: [f32; 4],
    /// Blend width (pixels, or radians on the globe sky), globe transition, viewport height
    /// (negative for the globe sky), and reversed sky depth.
    pub blend: [f32; 4],
    /// Inverse OpenGL-convention projection turning a clip position into a view-space ray.
    pub inverse_projection: Mat4x4f32,
    /// The eye's local zenith in view axes, then the elevation of its sea-level horizon in
    /// radians (zero or negative).
    pub up_and_horizon: [f32; 4],
}

impl SkyLayerMetadata {
    /// Shader location of [`Self::up_and_horizon`]; the sky's other attributes take 8..=15.
    const UP_AND_HORIZON_LOCATION: u32 = 7;
}

/// Shader filling the screen above the horizon with the sky, as GL JS `drawSky` does.
pub struct SkyShader {
    /// Render-target format.
    pub format: wgpu::TextureFormat,
}

fn sky_vertex_attribute(index: u64) -> wgpu::VertexAttribute {
    wgpu::VertexAttribute {
        offset: index * wgpu::VertexFormat::Float32x4.size(),
        format: wgpu::VertexFormat::Float32x4,
        shader_location: if index < 8 {
            8 + index as u32
        } else {
            SkyLayerMetadata::UP_AND_HORIZON_LOCATION
        },
    }
}

fn premultiplied_color_target(format: wgpu::TextureFormat) -> Vec<Option<wgpu::ColorTargetState>> {
    vec![Some(wgpu::ColorTargetState {
        format,
        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        write_mask: wgpu::ColorWrites::ALL,
    })]
}

impl Shader for SkyShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/engine/render/gpu/shaders/sky.vertex.wgsl"
            )),
            entry_point: "main",
            buffers: vec![VertexBufferLayout {
                array_stride: std::mem::size_of::<SkyLayerMetadata>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: (0..9).map(sky_vertex_attribute).collect(),
            }],
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/engine/render/gpu/shaders/sky.fragment.wgsl"
            )),
            entry_point: "main",
            targets: premultiplied_color_target(self.format),
        }
    }
}

/// Shader drawing a translucent atmospheric shell.
pub struct AtmosphereShader {
    /// Render-target format.
    pub format: wgpu::TextureFormat,
}

impl Shader for AtmosphereShader {
    fn describe_vertex(&self) -> VertexState {
        VertexState {
            source: include_str!(concat!(
                env!("OUT_DIR"),
                "/wgsl/engine/render/gpu/shaders/atmosphere.vertex.wgsl"
            )),
            entry_point: "main",
            buffers: atmosphere_buffers(),
        }
    }

    fn describe_fragment(&self) -> FragmentState {
        FragmentState {
            source: concat!(
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/engine/render/gpu/shaders/atmosphere_scattering.wgsl.part"
                )),
                include_str!(concat!(
                    env!("OUT_DIR"),
                    "/wgsl/engine/render/gpu/shaders/atmosphere.fragment.wgsl"
                ))
            ),
            entry_point: "main",
            // The scattering shader emits premultiplied RGB, matching GL JS's
            // ColorMode.alphaBlended (`ONE`, `ONE_MINUS_SRC_ALPHA`).
            targets: premultiplied_color_target(self.format),
        }
    }
}

fn matrix_attribute(column: u64, shader_location: u32) -> wgpu::VertexAttribute {
    wgpu::VertexAttribute {
        offset: column * wgpu::VertexFormat::Float32x4.size(),
        format: wgpu::VertexFormat::Float32x4,
        shader_location,
    }
}

fn atmosphere_buffers() -> Vec<VertexBufferLayout> {
    vec![VertexBufferLayout {
        array_stride: std::mem::size_of::<AtmosphereLayerMetadata>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: vec![
            matrix_attribute(0, 8),
            matrix_attribute(1, 9),
            matrix_attribute(2, 10),
            matrix_attribute(3, 11),
            wgpu::VertexAttribute {
                offset: 4 * wgpu::VertexFormat::Float32x4.size(),
                format: wgpu::VertexFormat::Float32x4,
                shader_location: 12,
            },
            wgpu::VertexAttribute {
                offset: 5 * wgpu::VertexFormat::Float32x4.size(),
                format: wgpu::VertexFormat::Float32x4,
                shader_location: 13,
            },
            wgpu::VertexAttribute {
                offset: 6 * wgpu::VertexFormat::Float32x4.size(),
                format: wgpu::VertexFormat::Float32x4,
                shader_location: 14,
            },
        ],
    }]
}
