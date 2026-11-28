//! Screen coordinates and ground intersections of the map view.
use super::*;
use crate::render::camera::ViewProjectionError;

impl ViewState {
    /// Inverts the camera and clip transforms separately to preserve a small near plane
    /// alongside Mercator coordinates millions of world pixels from the origin.
    pub fn inverted_view_projection(&self) -> Result<InvertedViewProjection, ViewProjectionError> {
        let camera = ViewProjection(self.camera_matrix())
            .invert()?
            .clip_to_camera;
        let depth = ViewProjection(OPENGL_TO_WGPU_MATRIX)
            .invert()?
            .clip_to_camera;
        let clip = match self.external_projection() {
            Some(projection) => {
                FLIP_Y * ViewProjection(projection).invert()?.clip_to_camera * depth
            }
            None => {
                ViewProjection(self.perspective_matrix())
                    .invert()?
                    .clip_to_camera
                    * depth
                    * FLIP_Y
            }
        };
        Ok(InvertedViewProjection {
            clip_to_camera: clip,
            camera_to_world: camera,
        })
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

    #[inline]
    fn window_point_to_ndc(&self, window: &Vector3<f64>) -> Vector4<f64> {
        Vector4::new(
            2.0 * window.x / self.width - 1.0,
            1.0 - 2.0 * window.y / self.height,
            window.z,
            1.0,
        )
    }

    #[inline]
    fn ray_plane_intersection_t(near_z: f64, far_z: f64, target_z: f64) -> Option<f64> {
        let dz = far_z - near_z;
        (dz.abs() > f64::EPSILON).then_some((target_z - near_z) / dz)
    }

    fn project_ground_ndc_plane(&self) -> Option<Plane<f64>> {
        let view_proj = self.view_projection();
        let a = view_proj.project(Vector4::new(0.0, 0.0, 0.0, 1.0));
        let b = view_proj.project(Vector4::new(1.0, 0.0, 0.0, 1.0));
        let c = view_proj.project(Vector4::new(1.0, 1.0, 0.0, 1.0));

        let to_ndc = Vector3::new(1.0 / self.width, 1.0 / self.height, 1.0);
        Plane::from_points(
            Point3::from_vec(self.clip_to_window(&a).truncate().mul_element_wise(to_ndc)),
            Point3::from_vec(self.clip_to_window(&b).truncate().mul_element_wise(to_ndc)),
            Point3::from_vec(self.clip_to_window(&c).truncate().mul_element_wise(to_ndc)),
        )
    }

    fn bounds_from_world_points(points: &[Vector3<f64>]) -> Option<Aabb2<f64>> {
        let (min, max) = bounds_from_points(points.iter().map(|point| [point.x, point.y]))?;
        Some(Aabb2::new(Point2::from(min), Point2::from(max)))
    }

    #[inline]
    fn frustum_corner_window(&self, index: usize) -> Vector3<f64> {
        let (x, y) = match index & 3 {
            0 => (0.0, 0.0),
            1 => (self.width, 0.0),
            2 => (self.width, self.height),
            _ => (0.0, self.height),
        };
        let depth = if index < 4 { 1.0 } else { 0.0 };
        Vector3::new(x, y, depth)
    }

    /// Order of transformations reversed: <https://computergraphics.stackexchange.com/questions/6087/screen-space-coordinates-to-eye-space-conversion/6093>
    /// `w` is lost.
    ///
    /// OpenGL explanation: <https://www.khronos.org/opengl/wiki/Compute_eye_space_from_window_space#From_window_to_ndc>
    pub(super) fn window_to_world(
        &self,
        window: &Vector3<f64>,
        inverted_view_proj: &InvertedViewProjection,
    ) -> Vector3<f64> {
        let ndc = self.window_point_to_ndc(window);
        let unprojected = inverted_view_proj.project(ndc);

        Vector3::new(
            unprojected.x / unprojected.w,
            unprojected.y / unprojected.w,
            unprojected.z / unprojected.w,
        )
    }

    /// Unprojects a window position at a clip depth in `0..=1` into world pixels with metres of
    /// elevation on `z`.
    pub fn window_to_world_at_depth(
        &self,
        window: &Vector2<f64>,
        depth: f64,
        inverted_view_proj: &InvertedViewProjection,
    ) -> Vector3<f64> {
        self.window_to_world(&Vector3::new(window.x, window.y, depth), inverted_view_proj)
    }

    /// Gets the world coordinates for the specified `window` coordinates on the horizontal plane
    /// `elevation` metres above sea level.
    pub fn window_to_world_at_elevation(
        &self,
        window: &Vector2<f64>,
        inverted_view_proj: &InvertedViewProjection,
        elevation: f64,
    ) -> Option<Vector2<f64>> {
        let near_world = self.window_to_world_at_depth(window, 0.0, inverted_view_proj);
        let far_world = self.window_to_world_at_depth(window, 1.0, inverted_view_proj);
        let u = Self::ray_plane_intersection_t(near_world.z, far_world.z, elevation)?;
        let result = near_world + u * (far_world - near_world);
        (result.x.is_finite() && result.y.is_finite()).then_some(Vector2::new(result.x, result.y))
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
        let u = Self::ray_plane_intersection_t(near_world.z, far_world.z, 0.0).unwrap_or(f64::NAN);
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
        let plane = self.project_ground_ndc_plane()?;
        let points = plane.intersection_points_aabb3(&Aabb3::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 1.0),
        ));

        let inverted_view_proj = self.inverted_view_projection().ok()?;
        let from_ndc = Vector3::new(self.width, self.height, 1.0);
        let world_points: Vec<_> = points
            .iter()
            .map(|point| {
                self.window_to_world(&point.mul_element_wise(from_ndc), &inverted_view_proj)
            })
            .collect();
        Self::bounds_from_world_points(&world_points)
    }

    /// Corners of the view frustum in world space using an already-inverted clip-to-world transform.
    pub(crate) fn frustum_corners_with_inverted(
        &self,
        inverted: &InvertedViewProjection,
    ) -> Result<[Vector3<f64>; 8], ViewProjectionError> {
        let points = std::array::from_fn(|index| {
            self.window_to_world(&self.frustum_corner_window(index), inverted)
        });
        if points
            .iter()
            .any(|point| !point.x.is_finite() || !point.y.is_finite() || !point.z.is_finite())
        {
            return Err(ViewProjectionError);
        }
        Ok(points)
    }

    /// Corners of the view frustum in world space: four on the far plane, then four on the
    /// near plane, each ordered top-left, top-right, bottom-right, bottom-left on screen.
    pub fn frustum_corners(&self) -> Result<[Vector3<f64>; 8], ViewProjectionError> {
        let inverted = self.inverted_view_projection()?;
        self.frustum_corners_with_inverted(&inverted)
    }
}
