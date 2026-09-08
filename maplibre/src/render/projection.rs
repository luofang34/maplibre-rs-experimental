//! GPU-facing projection data shared by tile shaders.

use crate::{
    coords::{LatLon, TILE_SIZE},
    projection::{
        body::Body,
        globe::{
            camera::{GlobeCameraError, GlobeCameraOptions, GlobeCameraState},
            covering_tiles::GlobeCoveringError,
            lat_lon_to_unit_sphere,
        },
        mercator::MercatorCoveringError,
        renderer_data::{
            compose_projection_data, ProjectionDataParams, ProjectionMatrices,
            RendererProjectionData,
        },
    },
    render::{
        render_phase::ProjectionBinding,
        shaders::{Mat4x4f32, Vec4f32},
        view_state::ViewState,
    },
    style::Style,
};
use bytemuck_derive::{Pod, Zeroable};
use cgmath::{Matrix4, Point2};
use thiserror::Error;
use wgpu::util::DeviceExt;
mod covering;
pub use covering::{
    covering_region, raster_source_regions, view_region_for_projection, CoveringRequest,
};

/// View-wide globe projection values uploaded as a uniform buffer.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct ShaderProjectionData {
    /// Matrix projecting the unit globe to WebGPU clip space.
    pub main_matrix: Mat4x4f32,
    /// Unit-sphere horizon plane.
    pub clipping_plane: Vec4f32,
    /// Mercator-to-globe interpolation factor.
    pub transition: f32,
    /// Clip-space w of the view center: the camera-to-center distance in Mercator pixels,
    /// blended with the globe's value, so screen-space sizes can scale with distance.
    pub center_clip_w: f32,
    /// Radius of the body in metres, which scales elevations onto the unit sphere.
    pub radius_meters: f32,
    padding: f32,
}

impl ShaderProjectionData {
    /// Creates the view-wide subset of renderer projection data.
    pub fn from_renderer_data(data: RendererProjectionData) -> Self {
        Self {
            main_matrix: data.main_matrix.into(),
            clipping_plane: data.clipping_plane.into(),
            transition: data.projection_transition,
            center_clip_w: 1.0,
            radius_meters: Body::EARTH.radius_meters as f32,
            padding: 0.0,
        }
    }
}

impl Default for ShaderProjectionData {
    fn default() -> Self {
        Self {
            main_matrix: Matrix4::from_scale(1.0).into(),
            clipping_plane: [0.0, 0.0, 0.0, 1.0],
            transition: 0.0,
            center_clip_w: 1.0,
            radius_meters: Body::EARTH.radius_meters as f32,
            padding: 0.0,
        }
    }
}

/// GPU buffer and binding shared by projection-aware pipelines.
pub struct ProjectionGpuResources {
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    buffer: wgpu::Buffer,
    /// Never-updated default projection for draws into drape textures.
    flat_bind_group: wgpu::BindGroup,
}

impl ProjectionGpuResources {
    /// Allocates the projection uniform and its stable bind-group layout.
    pub fn new(device: &wgpu::Device) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("projection uniform layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<ShaderProjectionData>() as u64,
                    ),
                },
                count: None,
            }],
        });
        let initial_data = ShaderProjectionData::default();
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("projection uniform buffer"),
            contents: bytemuck::bytes_of(&initial_data),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("projection uniform bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        let flat_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("flat projection uniform buffer"),
            contents: bytemuck::bytes_of(&initial_data),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let flat_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flat projection uniform bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: flat_buffer.as_entire_binding(),
            }],
        });
        Self {
            bind_group_layout,
            bind_group,
            buffer,
            flat_bind_group,
        }
    }

    /// Returns the bind group a draw asks for: the camera's projection or the flat default.
    pub fn bind_group_for(&self, binding: ProjectionBinding) -> &wgpu::BindGroup {
        match binding {
            ProjectionBinding::View => &self.bind_group,
            ProjectionBinding::Flat => &self.flat_bind_group,
        }
    }

    /// Returns the layout used when creating projection-aware pipelines.
    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.bind_group_layout
    }

    /// Returns the bind group used by projection-aware draw commands.
    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }

    /// Uploads projection state for the current frame.
    pub fn upload(&self, queue: &wgpu::Queue, data: ShaderProjectionData) {
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&data));
    }
}

/// Failure while deriving projection state from the current map view.
#[derive(Debug, Error)]
pub enum ProjectionStateError {
    /// Mercator tile selection failed.
    #[error("failed to select mercator tiles")]
    MercatorCovering {
        /// Underlying covering error.
        #[source]
        source: MercatorCoveringError,
    },
    /// Globe camera state could not be constructed.
    #[error("failed to construct globe camera state: {source}")]
    GlobeCamera {
        /// Underlying camera error.
        #[source]
        source: GlobeCameraError,
    },
    /// A globe matrix or clipping plane cannot be represented as 32-bit floats.
    #[error("globe projection state cannot be represented as f32")]
    FloatConversion,
    /// Globe tile traversal failed.
    #[error("failed to select visible globe tiles")]
    GlobeCovering {
        /// Underlying covering error.
        #[source]
        source: GlobeCoveringError,
    },
}

/// Derives the view-wide projection uniform from style and camera state.
pub fn projection_data_for_view(
    style: &Style,
    view_state: &ViewState,
) -> Result<ShaderProjectionData, ProjectionStateError> {
    let transition = style.projection.as_ref().map_or(0.0, |specification| {
        specification
            .projection_type
            .globe_transition(view_state.zoom().value())
    });
    let mercator_center_w = view_state.camera_to_center_distance() as f32;
    let radius_meters = view_state.body().radius_meters as f32;
    if transition == 0.0 {
        return Ok(ShaderProjectionData {
            center_clip_w: mercator_center_w,
            radius_meters,
            ..ShaderProjectionData::default()
        });
    }

    let globe = globe_camera_for_view(view_state)?;
    let globe_center_clip =
        globe.wgpu_view_projection() * lat_lon_to_unit_sphere(globe.center()).extend(1.0);
    let globe_center_w = globe_center_clip.w as f32;
    let globe_matrix = globe
        .wgpu_view_projection()
        .cast::<f32>()
        .ok_or(ProjectionStateError::FloatConversion)?;
    let clipping_plane = globe
        .clipping_plane()
        .cast::<f32>()
        .ok_or(ProjectionStateError::FloatConversion)?;
    let data = compose_projection_data(
        ProjectionMatrices {
            mercator: Matrix4::from_scale(1.0),
            globe: globe_matrix,
        },
        clipping_plane,
        transition,
        ProjectionDataParams {
            apply_globe_matrix: true,
            ..ProjectionDataParams::default()
        },
    );
    Ok(ShaderProjectionData {
        center_clip_w: mercator_center_w + (globe_center_w - mercator_center_w) * transition,
        radius_meters,
        ..ShaderProjectionData::from_renderer_data(data)
    })
}

/// Constructs the vertical-perspective camera matching the current map view.
pub fn globe_camera_for_view(
    view_state: &ViewState,
) -> Result<GlobeCameraState, ProjectionStateError> {
    let world_size = TILE_SIZE * 2.0_f64.powf(view_state.zoom().value());
    let camera_position = view_state.camera().position();
    let center = mercator_world_to_lat_lon(camera_position.x, camera_position.y, world_size);
    let external_eye = view_state.external_globe_eye();
    let options = GlobeCameraOptions {
        width: view_state.width(),
        height: view_state.height(),
        field_of_view_degrees: external_eye.map_or_else(
            || view_state.field_of_view().0.to_degrees(),
            |eye| eye.frustum.vertical_field_of_view().0.to_degrees(),
        ),
        center,
        world_size,
        bearing_degrees: view_state.camera().get_bearing().0.to_degrees(),
        pitch_degrees: view_state.camera().get_pitch().0.to_degrees(),
        roll_degrees: external_eye.map_or(0.0, |_| view_state.camera().get_roll().0.to_degrees()),
        center_offset: external_eye
            .map_or_else(|| view_state.center_offset(), |_| Point2::new(0.0, 0.0)),
        body: view_state.body(),
    };
    match external_eye {
        Some(eye) => GlobeCameraState::from_external_eye(options, eye),
        None => GlobeCameraState::new(options),
    }
    .map_err(|source| ProjectionStateError::GlobeCamera { source })
}

pub(crate) fn mercator_world_to_lat_lon(x: f64, y: f64, world_size: f64) -> LatLon {
    let longitude = x / world_size * 360.0 - 180.0;
    let latitude = (std::f64::consts::PI * (1.0 - 2.0 * y / world_size))
        .sinh()
        .atan()
        .to_degrees();
    LatLon::new(latitude, longitude)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "projection/coverage/tests.rs"]
mod coverage_tests;
