//! Screen coordinates and ground intersections of the map view.
use super::*;

impl ViewState {
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
    pub(super) fn clip_to_window_vulkan(&self, clip: &Vector4<f64>) -> Vector3<f64> {
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
    pub(super) fn window_to_world(
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

        let ndc = Vector4::new(
            2.0 * fixed_window.x / self.width - 1.0,
            1.0 - 2.0 * fixed_window.y / self.height,
            fixed_window.z,
            1.0,
        );
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
    pub(super) fn window_to_world_nalgebra(
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
        let dz = far_world.z - near_world.z;
        if dz.abs() <= f64::EPSILON {
            return None;
        }
        let u = (elevation - near_world.z) / dz;
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

        let inverted_view_proj = self.inverted_view_projection().ok()?;

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
            .min_by(|a, b| a.total_cmp(b))?;
        let min_y = vec
            .iter()
            .map(|point| point.y)
            .min_by(|a, b| a.total_cmp(b))?;
        let max_x = vec
            .iter()
            .map(|point| point.x)
            .max_by(|a, b| a.total_cmp(b))?;
        let max_y = vec
            .iter()
            .map(|point| point.y)
            .max_by(|a, b| a.total_cmp(b))?;
        Some(Aabb2::new(
            Point2::new(min_x, min_y),
            Point2::new(max_x, max_y),
        ))
    }
}
