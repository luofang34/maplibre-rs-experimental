//! Shader uniform packing (`ShaderProjectionData`) for Mercator and vertical-perspective globe views.

use bytemuck_derive::{Pod, Zeroable};
use cgmath::{InnerSpace, Matrix4, Vector4};

use crate::{
    coords::LatLon,
    projection::{
        body::Body,
        globe::{
            angular_radians_to_unit_sphere, camera::GlobeCameraState, elevate_surface_point,
            mercator_to_angular_radians, unit_sphere_to_lat_lon,
        },
        renderer_data::{
            compose_projection_data, ProjectionDataParams, ProjectionMatrices,
            RendererProjectionData,
        },
    },
    render::{
        camera::OPENGL_TO_WGPU_MATRIX,
        error::ProjectionStateError,
        shaders::{Mat4x4f32, Vec4f32},
        view_state::{NavigationMode, ViewState},
    },
};

/// The zoom from which the shaders project globe positions relative to the view's centre: a
/// pixel is about 20 m there, and the metres global f32 coordinates round to start to show.
pub(crate) const PRECISE_GLOBE_ZOOM: f64 = 12.0;

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

/// A freely placed camera has no canonical map center for depth-based label scaling.
pub(crate) fn fixed_symbol_scale(view: &ViewState) -> bool {
    view.has_external_view() || view.navigation_mode() == NavigationMode::FreeGlobe
}

pub(crate) fn flat_shader_projection(view_state: &ViewState) -> ShaderProjectionData {
    ShaderProjectionData {
        external_view: f32::from(view_state.has_external_view()),
        center_clip_w: view_state.camera_to_center_distance() as f32,
        globe_circle: [1.0, 0.0, f32::from(fixed_symbol_scale(view_state)), 0.0],
        radius_meters: view_state.body().radius_meters as f32,
        ..ShaderProjectionData::default()
    }
}

#[inline]
fn blend_center_clip_w(mercator_w: f32, globe_w: f32, transition: f32) -> f32 {
    mercator_w + (globe_w - mercator_w) * transition
}

fn globe_projection_data(
    transition: f32,
    globe: &GlobeCameraState,
) -> Result<RendererProjectionData, ProjectionStateError> {
    let globe_matrix = globe
        .wgpu_view_projection()
        .cast::<f32>()
        .ok_or(ProjectionStateError::FloatConversion)?;
    let clipping_plane = globe
        .clipping_plane()
        .cast::<f32>()
        .ok_or(ProjectionStateError::FloatConversion)?;
    Ok(compose_projection_data(
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
    ))
}

fn apply_globe_center_references(
    shader: ShaderProjectionData,
    view_state: &ViewState,
    globe: &GlobeCameraState,
) -> ShaderProjectionData {
    let references = (view_state.zoom().value() >= PRECISE_GLOBE_ZOOM
        || view_state.has_external_view())
    .then(|| globe_center_references(globe))
    .flatten();
    match references {
        Some([center, clip, radial]) => ShaderProjectionData {
            globe_circle: [shader.globe_circle[0], 1.0, shader.globe_circle[2], 0.0],
            globe_center: center,
            globe_center_clip: clip,
            globe_center_radial: radial,
            ..shader
        },
        None => shader,
    }
}

pub(crate) fn globe_shader_projection(
    view_state: &ViewState,
    transition: f32,
    globe: &GlobeCameraState,
) -> Result<ShaderProjectionData, ProjectionStateError> {
    let mercator_center_w = view_state.camera_to_center_distance() as f32;
    let radius_meters = view_state.body().radius_meters as f32;
    let globe_center_clip = globe.wgpu_view_projection() * globe.target().extend(1.0);
    let globe_center_w = globe_center_clip.w as f32;
    let data = globe_projection_data(transition, globe)?;
    let shader = ShaderProjectionData {
        external_view: f32::from(view_state.has_external_view()),
        center_clip_w: blend_center_clip_w(mercator_center_w, globe_center_w, transition),
        radius_meters,
        globe_circle: [
            globe.circle_radius_correction() as f32,
            0.0,
            f32::from(fixed_symbol_scale(view_state)),
            0.0,
        ],
        ..ShaderProjectionData::from_renderer_data(data)
    };
    Ok(apply_globe_center_references(shader, view_state, globe))
}

#[inline]
pub(super) fn globe_horizon_distance_for_surface(
    transition: f32,
    camera: &GlobeCameraState,
    surface: cgmath::Vector3<f64>,
    z_clipping_start: f64,
) -> f64 {
    let clipping_t =
        ((f64::from(transition) - z_clipping_start) / (1.0 - z_clipping_start)).clamp(0.0, 1.0);
    let plane = camera.clipping_plane();
    let dot = plane.truncate().dot(surface) + plane.w;
    (1.0 - clipping_t) + clipping_t * dot
}

pub(super) struct GlobePointProjection<'a> {
    pub(super) world_size: f64,
    pub(super) body: Body,
    pub(super) view_projection: &'a crate::render::camera::ViewProjection,
    pub(super) z_clipping_start: f64,
}

pub(super) fn project_globe_clip_point(
    resolved: GlobePointProjection<'_>,
    transition: f32,
    camera: &GlobeCameraState,
    [x, y]: [f64; 2],
    elevation_meters: f64,
) -> Option<Vector4<f64>> {
    let mx = x / resolved.world_size;
    let my = (y / resolved.world_size).clamp(0.0, 1.0);
    let (lon, lat) = mercator_to_angular_radians(mx, my);
    let surface = angular_radians_to_unit_sphere(lon, lat);
    let horizon_dist =
        globe_horizon_distance_for_surface(transition, camera, surface, resolved.z_clipping_start);
    if horizon_dist < 0.0 {
        return None;
    }
    let elevated = elevate_surface_point(surface, elevation_meters, resolved.body);
    let globe_clip = OPENGL_TO_WGPU_MATRIX * camera.view_projection() * elevated.extend(1.0);
    if transition >= 1.0 {
        Some(globe_clip)
    } else {
        let flat_clip = resolved
            .view_projection
            .project(Vector4::new(x, y, elevation_meters, 1.0));
        let t = f64::from(transition);
        Some(flat_clip * (1.0 - t) + globe_clip * t)
    }
}

fn globe_center_lat_lon(globe: &GlobeCameraState) -> LatLon {
    if globe.is_external_eye() {
        let mag = globe.camera_position().magnitude();
        if mag > 1e-12 {
            return unit_sphere_to_lat_lon(globe.camera_position() / mag);
        }
    }
    globe.center()
}

fn mercator_f32_surface_point(x_f32: f32, y_f32: f32) -> cgmath::Vector3<f64> {
    let (x, y) = (f64::from(x_f32), f64::from(y_f32));
    let longitude = x * std::f64::consts::TAU + std::f64::consts::PI;
    let tangent_half_latitude = (std::f64::consts::PI - y * std::f64::consts::TAU).exp();
    let denominator = tangent_half_latitude * tangent_half_latitude + 1.0;
    let sin_latitude = (tangent_half_latitude * tangent_half_latitude - 1.0) / denominator;
    let cos_latitude = 2.0 * tangent_half_latitude / denominator;
    cgmath::Vector3::new(
        longitude.sin() * cos_latitude,
        sin_latitude,
        longitude.cos() * cos_latitude,
    )
}

fn project_reference_clip(
    matrix: Matrix4<f64>,
    surface: cgmath::Vector3<f64>,
    w: f64,
) -> Option<[f32; 4]> {
    let clip = matrix * surface.extend(w);
    let lanes = [clip.x as f32, clip.y as f32, clip.z as f32, clip.w as f32];
    lanes.iter().all(|lane| lane.is_finite()).then_some(lanes)
}

fn globe_center_references(globe: &GlobeCameraState) -> Option<[[f32; 4]; 3]> {
    let center_ll = globe_center_lat_lon(globe);
    let lat_rad = center_ll
        .latitude
        .clamp(-89.999_999, 89.999_999)
        .to_radians();
    let x_unclamped = center_ll.longitude / 360.0 + 0.5;
    let y_unclamped = (1.0 - lat_rad.tan().asinh() / std::f64::consts::PI) * 0.5;
    let (x_f32, y_f32) = (x_unclamped as f32, y_unclamped as f32);
    let surface = mercator_f32_surface_point(x_f32, y_f32);
    let matrix = globe.wgpu_view_projection();
    Some([
        [x_f32, y_f32, 0.0, 0.0],
        project_reference_clip(matrix, surface, 1.0)?,
        project_reference_clip(matrix, surface, 0.0)?,
    ])
}
