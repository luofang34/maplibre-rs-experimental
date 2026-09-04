use std::{
    f64,
    ops::{Deref, DerefMut},
};

use cgmath::{prelude::*, *};

use crate::{
    coords::{ViewRegion, WorldCoords, Zoom, ZoomLevel, TILE_SIZE},
    projection::globe::EARTH_RADIUS_METERS,
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
}

impl ViewState {
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
        }
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
        world_size / (2.0 * f64::consts::PI * EARTH_RADIUS_METERS * latitude.cos())
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
        self.view_region_bounding_box(&self.view_projection().invert())
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
        let fov_above_center = self.perspective.fovy().0 * (0.5 + center_offset.y / self.height);
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
        let width = self.width;
        let height = self.height;

        let center = self.edge_insets.center(width, height);
        // Offset between wanted center and usual/normal center
        let center_offset = center - Vector2::new(width, height) / 2.0;

        let camera_to_center_distance = self.camera_to_center_distance();
        let pixels_per_meter = self.pixels_per_meter();

        // World z is metres above sea level: shift the center elevation to the orbit point,
        // then scale metres to pixels before the camera transform, as GL JS does.
        let camera_matrix = self.camera.calc_matrix(camera_to_center_distance)
            * Matrix4::from_nonuniform_scale(1.0, 1.0, pixels_per_meter)
            * Matrix4::from_translation(Vector3::new(0.0, 0.0, -self.center_elevation));

        let (near_z, far_z) = self.depth_range(center_offset);

        let perspective =
            self.perspective
                .calc_matrix_with_center(width, height, near_z, far_z, center_offset);

        // Apply camera and move camera away from ground
        let view_projection = perspective * camera_matrix;

        ViewProjection(FLIP_Y * OPENGL_TO_WGPU_MATRIX * view_projection)
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
        log::info!("zoom: {new_zoom}");
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

    /// A transform which can be used to transform between clip and window space.
    /// Adopted from [here](https://docs.microsoft.com/en-us/windows/win32/direct3d9/viewports-and-clipping#viewport-rectangle) (Direct3D).
    pub(crate) fn clip_to_window_transform(&self) -> Matrix4<f64> {
        let min_depth = 0.0;
        let max_depth = 1.0;
        let x = 0.0;
        let y = 0.0;
        let ox = x + self.width / 2.0;
        let oy = y + self.height / 2.0;
        let oz = min_depth;
        let pz = max_depth - min_depth;
        Matrix4::from_cols(
            Vector4::new(self.width / 2.0, 0.0, 0.0, 0.0),
            Vector4::new(0.0, -self.height / 2.0, 0.0, 0.0),
            Vector4::new(0.0, 0.0, pz, 0.0),
            Vector4::new(ox, oy, oz, 1.0),
        )
    }

    /// Transforms coordinates in clip space to window coordinates.
    ///
    /// Adopted from [here](https://docs.microsoft.com/en-us/windows/win32/dxtecharts/the-direct3d-transformation-pipeline) (Direct3D).
    pub(crate) fn clip_to_window(&self, clip: &Vector4<f64>) -> Vector4<f64> {
        #[rustfmt::skip]
        let ndc = Vector4::new(
            clip.x / clip.w,
            clip.y / clip.w,
            clip.z / clip.w,
            1.0
        );

        self.clip_to_window_transform() * ndc
    }

    /// The way how maplibre converts from clip to window space: https://github.com/maplibre/maplibre-native/blob/4add9ead08799577a37c465b8cb1266676b6c41e/src/mbgl/text/collision_index.cpp/#L437-L438
    pub(crate) fn clip_to_window_maplibre(&self, clip: &Vector4<f64>) -> Vector4<f64> {
        assert_eq!(clip.z, 0.0);
        return Vector4::new(
            ((clip.x / clip.w + 1.) / 2.) * self.width,
            ((-clip.y / clip.w + 1.) / 2.) * self.height,
            0.0,
            1.0,
        );
    }

    /// Alternative implementation to `clip_to_window`. Transforms coordinates in clip space to
    /// window coordinates.
    ///
    /// Adopted from [here](https://www.khronos.org/registry/vulkan/specs/1.2-extensions/man/html/VkViewport.html)
    /// and [here](https://matthewwellings.com/blog/the-new-vulkan-coordinate-system/) (Vulkan).
    fn clip_to_window_vulkan(&self, clip: &Vector4<f64>) -> Vector3<f64> {
        #[rustfmt::skip]
            let ndc = Vector4::new(
            clip.x / clip.w,
            clip.y / clip.w,
            clip.z / clip.w,
            1.0
        );

        let min_depth = 0.0;
        let max_depth = 1.0;

        let x = 0.0;
        let y = 0.0;
        let ox = x + self.width / 2.0;
        let oy = y + self.height / 2.0;
        let oz = min_depth;
        let px = self.width;
        let py = self.height;
        let pz = max_depth - min_depth;
        let xd = ndc.x;
        let yd = ndc.y;
        let zd = ndc.z;
        Vector3::new(px / 2.0 * xd + ox, py / 2.0 * yd + oy, pz * zd + oz)
    }

    /// Order of transformations reversed: https://computergraphics.stackexchange.com/questions/6087/screen-space-coordinates-to-eye-space-conversion/6093
    /// `w` is lost.
    ///
    /// OpenGL explanation: https://www.khronos.org/opengl/wiki/Compute_eye_space_from_window_space#From_window_to_ndc
    fn window_to_world(
        &self,
        window: &Vector3<f64>,
        inverted_view_proj: &InvertedViewProjection,
    ) -> Vector3<f64> {
        #[rustfmt::skip]
            let fixed_window = Vector4::new(
            window.x,
            window.y,
            window.z,
            1.0
        );

        let ndc = self.clip_to_window_transform().invert().unwrap() * fixed_window;
        let unprojected = inverted_view_proj.project(ndc);

        Vector3::new(
            unprojected.x / unprojected.w,
            unprojected.y / unprojected.w,
            unprojected.z / unprojected.w,
        )
    }

    /// Alternative implementation to `window_to_world`
    ///
    /// Adopted from [here](https://docs.rs/nalgebra-glm/latest/src/nalgebra_glm/ext/matrix_projection.rs.html#164-181).
    fn window_to_world_nalgebra(
        window: &Vector3<f64>,
        inverted_view_proj: &InvertedViewProjection,
        width: f64,
        height: f64,
    ) -> Vector3<f64> {
        let pt = Vector4::new(
            2.0 * (window.x - 0.0) / width - 1.0,
            2.0 * (height - window.y - 0.0) / height - 1.0,
            window.z,
            1.0,
        );
        let unprojected = inverted_view_proj.project(pt);

        Vector3::new(
            unprojected.x / unprojected.w,
            unprojected.y / unprojected.w,
            unprojected.z / unprojected.w,
        )
    }

    /// Gets the world coordinates for the specified `window` coordinates on the `z=0` plane.
    pub fn window_to_world_at_ground(
        &self,
        window: &Vector2<f64>,
        inverted_view_proj: &InvertedViewProjection,
        bound: bool,
    ) -> Option<Vector2<f64>> {
        let near_world =
            self.window_to_world(&Vector3::new(window.x, window.y, 0.0), inverted_view_proj);

        let far_world =
            self.window_to_world(&Vector3::new(window.x, window.y, 1.0), inverted_view_proj);

        // for z = 0 in world coordinates
        // Idea comes from: https://dondi.lmu.build/share/cg/unproject-explained.pdf
        let u = -near_world.z / (far_world.z - near_world.z);
        if !bound || (0.0..=1.01).contains(&u) {
            let result = near_world + u * (far_world - near_world);
            Some(Vector2::new(result.x, result.y))
        } else {
            None
        }
    }

    /// Calculates an [`Aabb2`] bounding box which contains at least the visible area on the `z=0`
    /// plane. One can think of it as being the bounding box of the geometry which forms the
    /// intersection between the viewing frustum and the `z=0` plane.
    ///
    /// This implementation works in the world 3D space. It casts rays from the corners of the
    /// window to calculate intersections points with the `z=0` plane. Then a bounding box is
    /// calculated.
    ///
    /// *Note:* It is possible that no such bounding box exists. This is the case if the `z=0` plane
    /// is not in view.
    pub fn view_region_bounding_box(
        &self,
        inverted_view_proj: &InvertedViewProjection,
    ) -> Option<Aabb2<f64>> {
        let screen_bounding_box = [
            Vector2::new(0.0, 0.0),
            Vector2::new(self.width, 0.0),
            Vector2::new(self.width, self.height),
            Vector2::new(0.0, self.height),
        ]
        .map(|point| self.window_to_world_at_ground(&point, inverted_view_proj, false));

        let (min, max) = bounds_from_points(
            screen_bounding_box
                .into_iter()
                .flatten()
                .map(|point| [point.x, point.y]),
        )?;

        Some(Aabb2::new(Point2::from(min), Point2::from(max)))
    }
    /// An alternative implementation for `view_region_bounding_box`.
    ///
    /// This implementation works in the NDC space. We are creating a plane in the world 3D space.
    /// Then we are transforming it to the NDC space. In NDC space it is easy to calculate
    /// the intersection points between an Aabb3 and a plane. The resulting Aabb2 is returned.
    pub fn view_region_bounding_box_ndc(&self) -> Option<Aabb2<f64>> {
        let view_proj = self.view_projection();
        let a = view_proj.project(Vector4::new(0.0, 0.0, 0.0, 1.0));
        let b = view_proj.project(Vector4::new(1.0, 0.0, 0.0, 1.0));
        let c = view_proj.project(Vector4::new(1.0, 1.0, 0.0, 1.0));

        let a_ndc = self.clip_to_window(&a).truncate();
        let b_ndc = self.clip_to_window(&b).truncate();
        let c_ndc = self.clip_to_window(&c).truncate();
        let to_ndc = Vector3::new(1.0 / self.width, 1.0 / self.height, 1.0);
        let plane: Plane<f64> = Plane::from_points(
            Point3::from_vec(a_ndc.mul_element_wise(to_ndc)),
            Point3::from_vec(b_ndc.mul_element_wise(to_ndc)),
            Point3::from_vec(c_ndc.mul_element_wise(to_ndc)),
        )?;

        let points = plane.intersection_points_aabb3(&Aabb3::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 1.0),
        ));

        let inverted_view_proj = view_proj.invert();

        let from_ndc = Vector3::new(self.width, self.height, 1.0);
        let vec = points
            .iter()
            .map(|point| {
                self.window_to_world(&point.mul_element_wise(from_ndc), &inverted_view_proj)
            })
            .collect::<Vec<_>>();

        let min_x = vec
            .iter()
            .map(|point| point.x)
            .min_by(|a, b| a.partial_cmp(b).unwrap())?;
        let min_y = vec
            .iter()
            .map(|point| point.y)
            .min_by(|a, b| a.partial_cmp(b).unwrap())?;
        let max_x = vec
            .iter()
            .map(|point| point.x)
            .max_by(|a, b| a.partial_cmp(b).unwrap())?;
        let max_y = vec
            .iter()
            .map(|point| point.y)
            .max_by(|a, b| a.partial_cmp(b).unwrap())?;
        Some(Aabb2::new(
            Point2::new(min_x, min_y),
            Point2::new(max_x, max_y),
        ))
    }
    pub fn height(&self) -> f64 {
        self.height
    }
    pub fn width(&self) -> f64 {
        self.width
    }
}

#[cfg(test)]
mod tests {
    use cgmath::{Deg, Matrix4, Vector2, Vector4};

    use crate::{
        coords::{WorldCoords, Zoom},
        render::view_state::ViewState,
        window::PhysicalSize,
    };

    #[test]
    fn conform_transformation() {
        let fov = Deg(60.0);
        let mut state = ViewState::new(
            PhysicalSize::new(800, 600).unwrap(),
            WorldCoords::at_ground(0.0, 0.0),
            Zoom::new(10.0),
            Deg(0.0),
            fov,
        );

        //state.furthest_distance(state.camera_to_center_distance(), Point2::new(0.0, 0.0));

        let projection = state.view_projection().invert();

        let bottom_left = state
            .window_to_world_at_ground(&Vector2::new(0.0, 0.0), &projection, true)
            .unwrap();
        println!("bottom left on ground {:?}", bottom_left);
        let top_right = state
            .window_to_world_at_ground(&Vector2::new(state.width, state.height), &projection, true)
            .unwrap();
        println!("top right on ground {:?}", top_right);

        let mut rotated = Matrix4::from_angle_x(Deg(-30.0))
            * Vector4::new(bottom_left.x, bottom_left.y, 0.0, 0.0);

        println!("bottom left rotated around x axis {:?}", rotated);

        rotated = Matrix4::from_angle_y(Deg(-30.0)) * rotated;

        println!("bottom left rotated around x and y axis {:?}", rotated);

        state.camera.set_pitch(Deg(30.0));
        //state.camera.set_yaw(Deg(-30.0));

        // TODO: verify far distance plane calculation
    }

    fn state_at(zoom: f64, pitch: Deg<f64>) -> ViewState {
        ViewState::new(
            PhysicalSize::new(800, 600).unwrap(),
            WorldCoords::at_ground(256.0 * 2.0_f64.powf(zoom), 256.0 * 2.0_f64.powf(zoom)),
            Zoom::new(zoom),
            pitch,
            Deg(36.87),
        )
    }

    #[test]
    fn pixels_per_meter_follows_world_size_at_the_equator() {
        let state = state_at(0.0, Deg(0.0));
        let expected = 512.0 / (2.0 * std::f64::consts::PI * 6_371_008.8);

        assert!((state.pixels_per_meter() - expected).abs() < 1e-12);
        assert!((state_at(3.0, Deg(0.0)).pixels_per_meter() - expected * 8.0).abs() < 1e-12);
    }

    #[test]
    fn center_elevation_keeps_the_elevated_center_on_screen_center() {
        let mut state = state_at(12.0, Deg(45.0));
        state.set_center_elevation(570.0);
        let center = state.camera.position();

        let clip = state
            .view_projection()
            .project(Vector4::new(center.x, center.y, 570.0, 1.0));
        let window = state.clip_to_window(&clip);

        assert!((window.x - 400.0).abs() < 1e-6);
        assert!((window.y - 300.0).abs() < 1e-6);
    }

    #[test]
    fn far_plane_stays_finite_at_high_pitch() {
        let mut state = state_at(12.0, Deg(0.0));
        state.set_max_pitch(Deg(85.0));
        state.camera_mut().set_pitch(Deg(85.0));
        let (near, far) = state.depth_range(cgmath::Point2::new(0.0, 0.0));

        assert!(near > 0.0);
        assert!(far.is_finite());
        assert!(far > state.camera_to_center_distance());
        assert!((state.camera.get_pitch().0 - 85.0_f64.to_radians()).abs() < 1e-9);
    }

    #[test]
    fn far_plane_matches_the_camera_distance_when_flat() {
        let state = state_at(5.0, Deg(0.0));
        let (_, far) = state.depth_range(cgmath::Point2::new(0.0, 0.0));

        assert!((far - state.camera_to_center_distance() * 1.01).abs() < 1e-6);
    }

    #[test]
    fn max_pitch_clamps_pitch_changes() {
        let mut state = state_at(5.0, Deg(70.0));
        assert!((state.camera.get_pitch().0 - 60.0_f64.to_radians()).abs() < 1e-9);

        state.set_max_pitch(Deg(85.0));
        state.camera_mut().set_pitch(Deg(90.0));
        assert!((state.camera.get_pitch().0 - 85.0_f64.to_radians()).abs() < 1e-9);
    }

    #[test]
    fn gpu_view_projection_reverses_depth_only() {
        let state = ViewState::new(
            PhysicalSize::new(800, 600).unwrap(),
            WorldCoords::at_ground(1024.0, 2048.0),
            Zoom::new(10.0),
            Deg(25.0),
            Deg(60.0),
        );
        let point = Vector4::new(1000.0, 2100.0, 0.0, 1.0);

        let cpu = state.view_projection().project(point);
        let gpu = state.gpu_view_projection().project(point);

        assert!((cpu.x - gpu.x).abs() < 1e-9);
        assert!((cpu.y - gpu.y).abs() < 1e-9);
        assert!((cpu.w - gpu.w).abs() < 1e-9);
        assert!((gpu.z / gpu.w - (1.0 - cpu.z / cpu.w)).abs() < 1e-9);
    }
}
