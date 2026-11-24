//! Unprojection without cancellation between world translation and perspective depth.

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

    /// Corners of the view frustum in world space using an already-inverted clip-to-world transform.
    pub(crate) fn frustum_corners_with_inverted(
        &self,
        inverted: &InvertedViewProjection,
    ) -> Result<[Vector3<f64>; 8], ViewProjectionError> {
        let points = std::array::from_fn(|index| {
            let (x, y) = match index & 3 {
                0 => (0.0, 0.0),
                1 => (self.width, 0.0),
                2 => (self.width, self.height),
                _ => (0.0, self.height),
            };
            let depth = if index < 4 { 1.0 } else { 0.0 };
            self.window_to_world(&Vector3::new(x, y, depth), inverted)
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
