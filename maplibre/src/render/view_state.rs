use std::{
    f64,
    ops::{Deref, DerefMut},
};

use cgmath::{prelude::*, *};

use crate::{
    coords::{ViewRegion, WorldCoords, Zoom, ZoomLevel, TILE_SIZE},
    projection::body::Body,
    render::camera::{
        Camera, EdgeInsets, InvertedViewProjection, Perspective, ViewProjection, FLIP_Y,
        OPENGL_TO_WGPU_MATRIX, REVERSED_Z,
    },
    util::{
        math::{bounds_from_points, Aabb2, Aabb3, Plane},
        ChangeObserver,
    },
    window::{LogicalSize, PhysicalSize},
};

const VIEW_REGION_PADDING: i32 = 1;
const MAX_N_TILES: usize = 512;
/// Pitch beyond which the Mercator plane has no usable horizon.
const MAX_MERCATOR_HORIZON_ANGLE: Rad<f64> = Rad(89.25 * f64::consts::PI / 180.0);
/// Keeps some scene below the camera renderable when terrain dips under it.
const MIN_RENDER_DISTANCE_BELOW_CAMERA_METERS: f64 = 100.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewStatePadding {
    // This is helpful for loading a set of tiles.
    Loose,
    // This is helpful for rendering a set of tiles.
    Tight,
}

#[derive(Clone)] // TODO: Remove
pub struct ViewState {
    zoom: ChangeObserver<Zoom>,
    camera: ChangeObserver<Camera>,
    perspective: Perspective,

    width: f64,
    height: f64,
    edge_insets: EdgeInsets,
    /// Terrain elevation in metres at the map center; the camera orbits this point.
    center_elevation: f64,
    /// Lowest terrain elevation in metres among visible tiles, used for the far plane.
    min_elevation: f64,
    /// Whether a gesture holds the center elevation still, as GL JS `elevationFreeze` does.
    center_elevation_frozen: bool,
    /// The body the map is drawn on; its radius turns metres into pixels.
    body: Body,
    /// The eye a host supplied, which replaces the map's perspective and, on the globe, its
    /// camera.
    external_eye: Option<ExternalEye>,
    /// Factor on the external eye's frustum tangents for tile requests.
    request_overscan: f64,
    opaque_environment: bool,
}

impl ViewState {
    /// Whether an external view replaces its surroundings and needs continuous sky coverage.
    pub fn opaque_environment(&self) -> bool {
        self.has_external_view() && self.opaque_environment
    }

    /// Sets the host's background coverage intent independently of the map projection.
    pub fn set_opaque_environment(&mut self, opaque: bool) {
        self.opaque_environment = opaque;
    }

    pub fn new<F: Into<Rad<f64>>, P: Into<Deg<f64>>>(
        window_size: PhysicalSize,
        position: WorldCoords,
        zoom: Zoom,
        pitch: P,
        fovy: F,
    ) -> Self {
        let camera = Camera::new((position.x, position.y), Deg(0.0), pitch.into());

        let perspective = Perspective::new(fovy);

        Self {
            zoom: ChangeObserver::new(zoom),
            camera: ChangeObserver::new(camera),
            perspective,
            width: window_size.width() as f64,
            height: window_size.height() as f64,
            edge_insets: EdgeInsets {
                top: 0.0,
                bottom: 0.0,
                left: 0.0,
                right: 0.0,
            },
            center_elevation: 0.0,
            min_elevation: 0.0,
            center_elevation_frozen: false,
            body: Body::default(),
            external_eye: None,
            request_overscan: 1.0,
            opaque_environment: false,
        }
    }

    /// Sets how far beyond an external eye's frustum tiles are requested, as a factor on its
    /// tangents; one requests what the frame shows.
    pub fn set_request_overscan(&mut self, factor: f64) {
        self.request_overscan = if factor.is_finite() && factor >= 1.0 {
            factor
        } else {
            1.0
        };
    }

    /// How far beyond an external eye's frustum tiles are requested.
    pub fn request_overscan(&self) -> f64 {
        self.request_overscan
    }

    /// The body the map is drawn on.
    pub fn body(&self) -> Body {
        self.body
    }

    /// Draws the map on another body; every metre-to-pixel conversion follows its radius.
    pub fn set_body(&mut self, body: Body) {
        self.body = body;
    }

    /// Terrain elevation in metres at the map center.
    pub fn center_elevation(&self) -> f64 {
        self.center_elevation
    }

    /// Sets the terrain elevation at the map center; the camera keeps its distance to it.
    pub fn set_center_elevation(&mut self, meters: f64) {
        if meters.is_finite() {
            self.center_elevation = meters;
        }
    }

    /// Holds the center elevation still until [`thaw_center_elevation`](Self::thaw_center_elevation).
    ///
    /// The camera would otherwise bob with every change of the terrain under the center while a
    /// drag or zoom is in progress.
    pub fn freeze_center_elevation(&mut self) {
        self.center_elevation_frozen = true;
    }

    /// Lets the center elevation follow the terrain again.
    pub fn thaw_center_elevation(&mut self) {
        self.center_elevation_frozen = false;
    }

    /// Whether a gesture currently holds the center elevation still.
    pub fn center_elevation_frozen(&self) -> bool {
        self.center_elevation_frozen
    }

    /// Sets the lowest visible terrain elevation, which extends the far plane below sea level.
    pub fn set_min_elevation(&mut self, meters: f64) {
        if meters.is_finite() {
            self.min_elevation = meters;
        }
    }

    /// Sets the largest pitch the camera accepts.
    pub fn set_max_pitch<P: Into<Rad<f64>>>(&mut self, max_pitch: P) {
        self.camera.set_max_pitch(max_pitch);
    }

    /// Pixels per metre at the map center, so elevation in metres maps onto world pixels.
    pub fn pixels_per_meter(&self) -> f64 {
        let world_size = TILE_SIZE * 2.0_f64.powf(self.zoom.value());
        let latitude = (f64::consts::PI * (1.0 - 2.0 * self.camera.position().y / world_size))
            .sinh()
            .atan();
        world_size / (self.body.circumference_meters() * latitude.cos())
    }
    pub fn set_edge_insets(&mut self, edge_insets: EdgeInsets) {
        self.edge_insets = edge_insets;
    }

    pub fn edge_insets(&self) -> &EdgeInsets {
        &self.edge_insets
    }

    pub fn resize(&mut self, size: LogicalSize) {
        self.width = size.width() as f64;
        self.height = size.height() as f64;
    }

    pub fn create_view_region(
        &self,
        visible_level: ZoomLevel,
        padding: ViewStatePadding,
    ) -> Option<ViewRegion> {
        self.view_region_bounding_box(&self.inverted_view_projection().ok()?)
            .map(|bounding_box| {
                ViewRegion::new(
                    bounding_box,
                    match padding {
                        ViewStatePadding::Loose => VIEW_REGION_PADDING,
                        ViewStatePadding::Tight => 0,
                    },
                    MAX_N_TILES,
                    *self.zoom,
                    visible_level,
                )
            })
    }

    /// Distance in pixels from the screen center to the Mercator horizon, positive above the
    /// center, as GL JS `getMercatorHorizon`.
    pub fn mercator_horizon(&self) -> f64 {
        let pitch = self.camera.get_pitch().0.abs();
        self.camera_to_center_distance()
            * ((f64::consts::FRAC_PI_2 - pitch).tan() * 0.85)
                .min((MAX_MERCATOR_HORIZON_ANGLE.0 - pitch).tan())
    }

    /// Distances in pixels the fog depth spans: from the camera's distance to sea level, where
    /// the map center sits, to the far plane, as the GL JS fog matrix takes them. An external
    /// eye's fog is fixed by its height instead, see
    /// [`eye_fog_depth_range`](Self::eye_fog_depth_range).
    pub fn fog_depth_range(&self) -> (f64, f64) {
        if let Some(range) = self.eye_fog_depth_range() {
            return range;
        }
        let distance = self.camera_to_center_distance();
        let limited_pitch = self
            .camera
            .get_pitch()
            .0
            .abs()
            .min(MAX_MERCATOR_HORIZON_ANGLE.0);
        let camera_to_sea_level = (distance / 2.0)
            .max(distance + self.center_elevation * self.pixels_per_meter() / limited_pitch.cos());
        let (_, far_z) = self.depth_range(self.center_offset());
        (camera_to_sea_level, far_z)
    }

    /// How much of the fog shows: GL JS fades it in as the horizon comes into view between 60
    /// and 70 degrees of pitch, while an external eye's wide frame has the horizon in view at
    /// almost any pitch, so its fog is always on.
    pub fn fog_opacity(&self) -> f32 {
        if self.external_eye.is_some() {
            return 1.0;
        }
        crate::style::sky::SkySpecification::fog_blend_opacity(
            self.camera.get_pitch().0.to_degrees(),
        )
    }

    /// Near and far clip distances in pixels, following GL JS `_calculateNearFarZIfNeeded`.
    ///
    /// The far plane reaches the top of the screen on the lowest visible plane, capped at the
    /// Mercator horizon so a pitched camera never asks for an infinite range.
    pub fn depth_range(&self, center_offset: Point2<f64>) -> (f64, f64) {
        let distance = self.camera_to_center_distance();
        let pixels_per_meter = self.pixels_per_meter();
        let pitch = self.camera.get_pitch().0.abs();
        let limited_pitch = pitch.min(MAX_MERCATOR_HORIZON_ANGLE.0);
        let camera_to_sea_level = (distance / 2.0)
            .max(distance + self.center_elevation * pixels_per_meter / limited_pitch.cos());
        let camera_altitude = pitch.cos() * distance / pixels_per_meter + self.center_elevation;
        let min_elevation = self
            .center_elevation
            .min(self.min_elevation)
            .min(camera_altitude - MIN_RENDER_DISTANCE_BELOW_CAMERA_METERS);
        let lowest_plane = if min_elevation < 0.0 {
            camera_to_sea_level - min_elevation * pixels_per_meter / limited_pitch.cos()
        } else {
            camera_to_sea_level
        };

        let ground_angle = f64::consts::FRAC_PI_2 + pitch;
        // A rolled view reaches further along the screen diagonal than the vertical field of
        // view alone, so the far plane widens the field of view with the roll as GL JS does.
        let roll = self.camera.get_roll().0;
        let rolled_fov = self.perspective.fovy().0
            * (roll.cos().abs() * self.height + roll.sin().abs() * self.width)
            / self.height;
        let fov_above_center = rolled_fov * (0.5 + center_offset.y / self.height);
        let surface_distance = |fov: f64| {
            fov.sin() * lowest_plane
                / (f64::consts::PI - ground_angle - fov)
                    .clamp(0.01, f64::consts::PI - 0.01)
                    .sin()
        };
        let top_half_surface_distance = surface_distance(fov_above_center);

        let horizon = distance
            * ((f64::consts::FRAC_PI_2 - pitch).tan() * 0.85)
                .min((MAX_MERCATOR_HORIZON_ANGLE.0 - pitch).tan());
        let horizon_angle = (horizon / distance).atan();
        let min_fov_center_to_horizon = f64::consts::FRAC_PI_2 - MAX_MERCATOR_HORIZON_ANGLE.0;
        let fov_center_to_horizon = if horizon_angle > min_fov_center_to_horizon {
            2.0 * horizon_angle * (0.5 + center_offset.y / (horizon * 2.0))
        } else {
            min_fov_center_to_horizon
        };
        let top_half_horizon_distance = surface_distance(fov_center_to_horizon);

        let top_half = top_half_surface_distance.min(top_half_horizon_distance);
        let far_z =
            ((f64::consts::FRAC_PI_2 - limited_pitch).cos() * top_half + lowest_plane) * 1.01;
        let near_z = self.height / 50.0;
        (near_z, far_z)
    }

    pub fn camera_to_center_distance(&self) -> f64 {
        let height = self.height;

        let fovy = self.perspective.fovy();
        let half_fovy = fovy / 2.0;

        // Camera height, such that given a certain field-of-view, exactly height/2 are visible on ground.
        let camera_to_center_distance = (height / 2.0) / (half_fovy.tan()); // We are using `height` here because this is the FOV in y direction (fovy).
        camera_to_center_distance
    }

    /// Returns the vertical field of view.
    pub fn field_of_view(&self) -> Rad<f64> {
        self.perspective.fovy()
    }

    /// Returns the perspective-center offset from the viewport center.
    pub fn center_offset(&self) -> Point2<f64> {
        let center = self.edge_insets.center(self.width, self.height);
        center - Vector2::new(self.width, self.height) / 2.0
    }

    /// This function matches how maplibre-gl-js implements perspective and cameras at the time
    /// of the mapbox -> maplibre fork: [src/geo/transform.ts#L680](https://github.com/maplibre/maplibre-gl-js/blob/e78ad7944ef768e67416daa4af86b0464bd0f617/src/geo/transform.ts#L680)
    #[tracing::instrument(skip_all)]
    pub fn view_projection(&self) -> ViewProjection {
        let camera_matrix = self.camera_matrix();
        match self.external_projection() {
            // The host's projection expects a camera space with y up; the map's camera space
            // has y down and flips the clip space afterwards instead.
            Some(projection) => {
                ViewProjection(OPENGL_TO_WGPU_MATRIX * projection * FLIP_Y * camera_matrix)
            }
            None => ViewProjection(
                FLIP_Y * OPENGL_TO_WGPU_MATRIX * self.perspective_matrix() * camera_matrix,
            ),
        }
    }

    /// World to camera space: world x and y in pixels, z in metres above sea level.
    ///
    /// World z is metres above sea level: shift the center elevation to the orbit point, then
    /// scale metres to pixels before the camera transform, as GL JS does.
    fn camera_matrix(&self) -> Matrix4<f64> {
        if let Some(external) = self.external_camera_matrix() {
            return external;
        }
        self.camera.calc_matrix(self.camera_to_center_distance())
            * Matrix4::from_nonuniform_scale(1.0, 1.0, self.pixels_per_meter())
            * Matrix4::from_translation(Vector3::new(0.0, 0.0, -self.center_elevation))
    }

    /// The map's own perspective in OpenGL clip conventions, its vanishing point moved by the
    /// edge insets.
    fn perspective_matrix(&self) -> Matrix4<f64> {
        let center_offset = self.center_offset();
        let (near_z, far_z) = self.depth_range(center_offset);
        self.perspective.calc_matrix_with_center(
            self.width,
            self.height,
            near_z,
            far_z,
            center_offset,
        )
    }

    /// The camera's position in world space: x and y in world pixels, z in metres above sea
    /// level, taken from the same transform that projects the scene so it agrees with the
    /// frustum whatever the camera's angles are.
    pub fn eye_position(&self) -> Vector3<f64> {
        let eye = self
            .camera_matrix()
            .invert()
            .map_or(Vector4::new(0.0, 0.0, 0.0, 1.0), |inverse| {
                inverse * Vector4::new(0.0, 0.0, 0.0, 1.0)
            });
        Vector3::new(eye.x / eye.w, eye.y / eye.w, eye.z / eye.w)
    }

    /// Returns the view projection in GPU clip conventions, which adds reversed-Z depth.
    ///
    /// CPU unprojection keeps [`Self::view_projection`], whose far plane sits at depth one.
    pub fn gpu_view_projection(&self) -> ViewProjection {
        ViewProjection(REVERSED_Z * self.view_projection().0)
    }

    pub fn zoom(&self) -> Zoom {
        *self.zoom
    }

    pub fn did_zoom_change(&self) -> bool {
        self.zoom.did_change(0.05)
    }

    pub fn update_zoom(&mut self, new_zoom: Zoom) {
        *self.zoom = new_zoom;
        tracing::debug!(zoom = new_zoom.value(), "zoom changed");
    }

    /// Changes the zoom while the map center stays on the same geographic location.
    ///
    /// The camera position is stored in world pixels of the current zoom, so it scales with
    /// the zoom change; [`update_zoom`](Self::update_zoom) leaves it as it is.
    pub fn zoom_to(&mut self, new_zoom: Zoom) {
        let scale = self.zoom.scale_delta(&new_zoom);
        let position = self.camera.position().to_vec() * scale;
        self.camera.move_to(Point2::from_vec(position));
        self.update_zoom(new_zoom);
    }

    pub fn camera(&self) -> &Camera {
        self.camera.deref()
    }

    pub fn camera_mut(&mut self) -> &mut Camera {
        self.camera.deref_mut()
    }

    pub fn did_camera_change(&self) -> bool {
        self.camera.did_change(0.05)
    }

    pub fn update_references(&mut self) {
        self.camera.update_reference();
        self.zoom.update_reference();
    }

    pub fn height(&self) -> f64 {
        self.height
    }
    pub fn width(&self) -> f64 {
        self.width
    }
}

mod external;
mod horizon;
mod pose;
mod screen;
mod unprojection;

use external::ExternalEye;
pub use external::{ExternalAnchor, ExternalView, ExternalViewError};
pub use horizon::HorizonLine;
pub use pose::CameraPose;

#[cfg(test)]
mod tests;
