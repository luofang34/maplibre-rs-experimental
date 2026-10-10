//! GPU-facing projection data shared by tile shaders.

use bytemuck_derive::{Pod, Zeroable};
use cgmath::{Matrix4, Point2};
use thiserror::Error;

use crate::{
    coords::{LatLon, TILE_SIZE},
    projection::{
        body::Body,
        globe::{
            camera::{GlobeCameraError, GlobeCameraOptions, GlobeCameraState},
            covering_tiles::GlobeCoveringError,
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
        view_state::{NavigationMode, ViewState},
    },
    style::Style,
};
mod covering;
pub use covering::{
    covering_region, raster_source_regions, terrain_region, view_region_for_projection,
    CoveringRequest,
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
    /// Whether viewport symbols use a world-up basis for a tracked external eye.
    pub external_view: f32,
    /// x: the globe's angle per pixel relative to a world of 512 pixels at the zoom, which the
    /// extent of a circle lying on the globe follows; y: 1 when the globe centre references
    /// below are set; z: 1 when symbol sizes are independent of target depth; w is padding.
    pub globe_circle: [f32; 4],
    /// The Mercator position of the globe centre the shaders project relative to, in x and y,
    /// exactly representable in f32 so tile origins difference from it without rounding; the
    /// other lanes are padding.
    pub globe_center: [f32; 4],
    /// Clip position of that centre on the unit sphere.
    pub globe_center_clip: [f32; 4],
    /// Clip-space vector from the body's centre to it, which a height scales.
    pub globe_center_radial: [f32; 4],
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
            external_view: 0.0,
            globe_circle: [1.0, 0.0, 0.0, 0.0],
            globe_center: [0.0; 4],
            globe_center_clip: [0.0; 4],
            globe_center_radial: [0.0; 4],
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
            external_view: 0.0,
            globe_circle: [1.0, 0.0, 0.0, 0.0],
            globe_center: [0.0; 4],
            globe_center_clip: [0.0; 4],
            globe_center_radial: [0.0; 4],
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
    pub fn new(device: &wgpu::Device, queue: &crate::render::upload_queue::UploadQueue) -> Self {
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
        let buffer = queue.create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("projection uniform buffer"),
                contents: bytemuck::bytes_of(&initial_data),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
        );
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("projection uniform bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        let flat_buffer = queue.create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("flat projection uniform buffer"),
                contents: bytemuck::bytes_of(&initial_data),
                usage: wgpu::BufferUsages::UNIFORM,
            },
        );
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
    pub fn upload(
        &self,
        queue: &crate::render::upload_queue::UploadQueue,
        data: ShaderProjectionData,
    ) {
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

/// A freely placed camera has no canonical map center for depth-based label scaling.
pub(crate) fn fixed_symbol_scale(view: &ViewState) -> bool {
    view.has_external_view() || view.navigation_mode() == NavigationMode::FreeGlobe
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
            external_view: f32::from(view_state.has_external_view()),
            center_clip_w: mercator_center_w,
            globe_circle: [1.0, 0.0, f32::from(fixed_symbol_scale(view_state)), 0.0],
            radius_meters,
            ..ShaderProjectionData::default()
        });
    }

    let globe = globe_camera_for_view(view_state)?;
    let globe_center_clip = globe.wgpu_view_projection() * globe.target().extend(1.0);
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
    let shader = ShaderProjectionData {
        external_view: f32::from(view_state.has_external_view()),
        center_clip_w: mercator_center_w + (globe_center_w - mercator_center_w) * transition,
        radius_meters,
        globe_circle: [
            globe.circle_radius_correction() as f32,
            0.0,
            f32::from(fixed_symbol_scale(view_state)),
            0.0,
        ],
        ..ShaderProjectionData::from_renderer_data(data)
    };
    // Below this zoom f32 rounding stays far under a pixel, and projecting from the global
    // coordinates keeps every edge where GL JS, which does the same, draws it.
    let references = (view_state.zoom().value() >= PRECISE_GLOBE_ZOOM)
        .then(|| globe_center_references(&globe))
        .flatten();
    Ok(match references {
        Some([center, clip, radial]) => ShaderProjectionData {
            globe_circle: [shader.globe_circle[0], 1.0, shader.globe_circle[2], 0.0],
            globe_center: center,
            globe_center_clip: clip,
            globe_center_radial: radial,
            ..shader
        },
        None => shader,
    })
}

/// The zoom from which the shaders project globe positions relative to the view's centre: a
/// pixel is about 20 m there, and the metres global f32 coordinates round to start to show.
const PRECISE_GLOBE_ZOOM: f64 = 12.0;

/// The centre the shaders project globe positions relative to: the f32 Mercator position
/// nearest the globe camera's centre on the ground, with its clip position and body-centre
/// vector computed in f64. Positions built from global f32 coordinates round to metres, which
/// a close view of the globe draws as geometry that slides; relative to this centre they keep
/// their separation.
fn globe_center_references(globe: &GlobeCameraState) -> Option<[[f32; 4]; 3]> {
    let center = crate::terrain::sightline::lat_lon_to_mercator(globe.center());
    // The centre need only be near the view; one f32 can hold exactly is differenced exactly.
    let (x_f32, y_f32) = (center.x as f32, center.y as f32);
    let (x, y) = (f64::from(x_f32), f64::from(y_f32));
    let longitude = x * std::f64::consts::TAU + std::f64::consts::PI;
    let tangent_half_latitude = (std::f64::consts::PI - y * std::f64::consts::TAU).exp();
    let denominator = tangent_half_latitude * tangent_half_latitude + 1.0;
    let sin_latitude = (tangent_half_latitude * tangent_half_latitude - 1.0) / denominator;
    let cos_latitude = 2.0 * tangent_half_latitude / denominator;
    let surface = cgmath::Vector3::new(
        longitude.sin() * cos_latitude,
        sin_latitude,
        longitude.cos() * cos_latitude,
    );
    let matrix = globe.wgpu_view_projection();
    let clip = |w: f64| {
        let clip = matrix * surface.extend(w);
        let lanes = [clip.x as f32, clip.y as f32, clip.z as f32, clip.w as f32];
        lanes.iter().all(|lane| lane.is_finite()).then_some(lanes)
    };
    Some([[x_f32, y_f32, 0.0, 0.0], clip(1.0)?, clip(0.0)?])
}

/// Constructs the vertical-perspective camera matching the current map view.
pub fn globe_camera_for_view(
    view_state: &ViewState,
) -> Result<GlobeCameraState, ProjectionStateError> {
    let external_eye = view_state.external_globe_eye();
    // A free-globe camera is drawn from its pose; a host's eye replaces either.
    let pose = view_state.pose_view().filter(|_| external_eye.is_none());
    let world_size = TILE_SIZE
        * 2.0_f64.powf(
            pose.as_ref()
                .map_or(view_state.zoom().value(), |pose| pose.style_zoom),
        );
    let camera_position = view_state.camera().position();
    let center = pose.as_ref().map_or_else(
        || mercator_world_to_lat_lon(camera_position.x, camera_position.y, world_size),
        |pose| pose.center,
    );
    let angle = |free: Option<f64>, flat: f64| free.unwrap_or(flat);
    let options = GlobeCameraOptions {
        width: view_state.width(),
        height: view_state.height(),
        field_of_view_degrees: external_eye.map_or_else(
            || view_state.field_of_view().0.to_degrees(),
            |eye| eye.frustum.vertical_field_of_view().0.to_degrees(),
        ),
        center,
        world_size,
        bearing_degrees: angle(
            pose.as_ref().map(|pose| pose.bearing_degrees),
            view_state.camera().get_bearing().0.to_degrees(),
        ),
        pitch_degrees: angle(
            pose.as_ref().map(|pose| pose.pitch_degrees),
            view_state.camera().get_pitch().0.to_degrees(),
        ),
        roll_degrees: angle(
            pose.as_ref().map(|pose| pose.roll_degrees),
            view_state.camera().get_roll().0.to_degrees(),
        ),
        center_offset: external_eye
            .map_or_else(|| view_state.center_offset(), |_| Point2::new(0.0, 0.0)),
        body: view_state.body(),
        // A host's eye is placed where it is; the center's terrain must not move it.
        target_elevation_meters: if external_eye.is_none() && view_state.globe_orbits_center() {
            view_state.center_elevation()
        } else {
            0.0
        },
        radius_pixels: pose.as_ref().map(|pose| pose.radius_pixels),
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
