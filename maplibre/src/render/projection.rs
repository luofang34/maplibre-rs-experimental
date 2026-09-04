//! GPU-facing projection data shared by tile shaders.

use bytemuck_derive::{Pod, Zeroable};
use cgmath::Matrix4;
use thiserror::Error;
use wgpu::util::DeviceExt;

use crate::{
    coords::{LatLon, ViewRegion, ZoomLevel, TILE_SIZE},
    projection::{
        globe::{
            camera::{GlobeCameraError, GlobeCameraOptions, GlobeCameraState},
            covering::TileElevationRange,
            covering_tiles::{
                covering_tiles, elevation_for_tile_culling, GlobeCoveringError,
                GlobeCoveringOptions,
            },
        },
        mercator::{
            covering_tiles as mercator_covering_tiles, MercatorCoveringError,
            MercatorCoveringOptions,
        },
        renderer_data::{
            compose_projection_data, ProjectionDataParams, ProjectionMatrices,
            RendererProjectionData,
        },
    },
    render::{
        render_phase::ProjectionBinding,
        shaders::{Mat4x4f32, Vec4f32},
        view_state::{ViewState, ViewStatePadding},
    },
    style::Style,
};

/// GL JS's feature-height allowance for tiles near the horizon.
const ASSUMED_MAX_FEATURE_HEIGHT_METERS: f64 = 500.0;
const MAX_MERCATOR_HORIZON_DEGREES: f64 = 89.25;
const TILE_CULLING_HORIZON_ONSET_DEGREES: f64 = 15.0;
/// Elevation range assumed for terrain tiles until DEM minima and maxima feed the covering.
const TERRAIN_MIN_ELEVATION_METERS: f64 = -500.0;
const TERRAIN_MAX_ELEVATION_METERS: f64 = 9000.0;

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
    padding: [f32; 3],
}

impl ShaderProjectionData {
    /// Creates the view-wide subset of renderer projection data.
    pub fn from_renderer_data(data: RendererProjectionData) -> Self {
        Self {
            main_matrix: data.main_matrix.into(),
            clipping_plane: data.clipping_plane.into(),
            transition: data.projection_transition,
            padding: [0.0; 3],
        }
    }
}

impl Default for ShaderProjectionData {
    fn default() -> Self {
        Self {
            main_matrix: Matrix4::from_scale(1.0).into(),
            clipping_plane: [0.0, 0.0, 0.0, 1.0],
            transition: 0.0,
            padding: [0.0; 3],
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
    #[error("failed to construct globe camera state")]
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
    if transition == 0.0 {
        return Ok(ShaderProjectionData::default());
    }

    let globe = globe_camera_for_view(view_state)?;
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
    Ok(ShaderProjectionData::from_renderer_data(data))
}

/// Selects the visible region using the projection declared by the current style.
pub fn view_region_for_projection(
    style: &Style,
    view_state: &ViewState,
    visible_level: ZoomLevel,
    padding: ViewStatePadding,
) -> Result<Option<ViewRegion>, ProjectionStateError> {
    let uses_globe = style.projection.as_ref().is_some_and(|specification| {
        specification
            .projection_type
            .uses_globe_rendering(view_state.zoom().value())
    });
    if !uses_globe {
        return mercator_view_region(style, view_state, visible_level, padding);
    }
    let camera = globe_camera_for_view(view_state)?;
    let tiles = covering_tiles(
        &camera,
        GlobeCoveringOptions {
            zoom: visible_level,
            requested_zoom: view_state.zoom().value(),
            variable_zoom: u8::from(visible_level) > 4,
            padding: match padding {
                ViewStatePadding::Loose => 1,
                ViewStatePadding::Tight => 0,
            },
            max_tiles: 512,
            elevation: TileElevationRange {
                min_meters: 0.0,
                max_meters: elevation_for_tile_culling(&camera, 0.0),
            },
        },
    )
    .map_err(|source| ProjectionStateError::GlobeCovering { source })?;
    Ok(Some(ViewRegion::from_tiles(tiles, visible_level, 512)))
}

/// Selects Mercator tiles: the ground-plane bounding box for flat views, or frustum culling
/// with distance-based zoom once terrain is on or the pitch exceeds GL JS's constant-zoom limit.
fn mercator_view_region(
    style: &Style,
    view_state: &ViewState,
    visible_level: ZoomLevel,
    padding: ViewStatePadding,
) -> Result<Option<ViewRegion>, ProjectionStateError> {
    let pitch_degrees = view_state.camera().get_pitch().0.to_degrees().abs();
    let fov_degrees = view_state.field_of_view().0.to_degrees();
    let needs_frustum =
        style.terrain.is_some() || pitch_degrees > max_constant_zoom_pitch(fov_degrees);
    if !needs_frustum {
        return Ok(view_state.create_view_region(visible_level, padding));
    }
    let tiles = mercator_covering_tiles(
        view_state,
        MercatorCoveringOptions {
            zoom: visible_level,
            requested_zoom: view_state.zoom().value(),
            variable_zoom: true,
            padding: match padding {
                ViewStatePadding::Loose => 1,
                ViewStatePadding::Tight => 0,
            },
            max_tiles: 512,
            elevation: mercator_elevation_range(style, view_state, pitch_degrees, fov_degrees),
        },
    )
    .map_err(|source| ProjectionStateError::MercatorCovering { source })?;
    Ok(Some(ViewRegion::from_tiles(tiles, visible_level, 512)))
}

/// Pitch above which GL JS varies zoom per tile: 78.5 degrees minus half the field of view,
/// clamped to 60 degrees.
fn max_constant_zoom_pitch(fov_degrees: f64) -> f64 {
    (78.5 - fov_degrees / 2.0).clamp(0.0, 60.0)
}

/// Elevation range of the tile boxes: the whole plausible range while terrain is on, else the
/// center elevation plus GL JS's feature-height allowance near the horizon.
fn mercator_elevation_range(
    style: &Style,
    view_state: &ViewState,
    pitch_degrees: f64,
    fov_degrees: f64,
) -> TileElevationRange {
    if style.terrain.is_some() {
        return TileElevationRange {
            min_meters: TERRAIN_MIN_ELEVATION_METERS,
            max_meters: TERRAIN_MAX_ELEVATION_METERS,
        };
    }
    let bottom_edge_above_horizontal =
        MAX_MERCATOR_HORIZON_DEGREES - pitch_degrees - fov_degrees * 0.5;
    let proximity = ((TILE_CULLING_HORIZON_ONSET_DEGREES - bottom_edge_above_horizontal)
        / TILE_CULLING_HORIZON_ONSET_DEGREES)
        .clamp(0.0, 1.0);
    let elevation = view_state.center_elevation() + proximity * ASSUMED_MAX_FEATURE_HEIGHT_METERS;
    TileElevationRange {
        min_meters: elevation.min(0.0),
        max_meters: elevation.max(0.0),
    }
}

/// Constructs the vertical-perspective camera matching the current map view.
pub fn globe_camera_for_view(
    view_state: &ViewState,
) -> Result<GlobeCameraState, ProjectionStateError> {
    let world_size = TILE_SIZE * 2.0_f64.powf(view_state.zoom().value());
    let camera_position = view_state.camera().position();
    let center = mercator_world_to_lat_lon(camera_position.x, camera_position.y, world_size);
    GlobeCameraState::new(GlobeCameraOptions {
        width: view_state.width(),
        height: view_state.height(),
        field_of_view_degrees: view_state.field_of_view().0.to_degrees(),
        center,
        world_size,
        bearing_degrees: view_state.camera().get_roll().0.to_degrees(),
        pitch_degrees: view_state.camera().get_pitch().0.to_degrees(),
        roll_degrees: 0.0,
        center_offset: view_state.center_offset(),
    })
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
