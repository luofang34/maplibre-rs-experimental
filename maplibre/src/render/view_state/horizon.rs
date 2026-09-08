//! The horizon on screen: where the flat map ends and the sky begins.

use cgmath::{EuclideanSpace, InnerSpace, Point2, SquareMatrix, Vector2, Vector4};

use super::ViewState;

/// A screen point far enough off any viewport that the whole frame lies on one side of it.
const OUT_OF_SIGHT_PIXELS: f64 = 1.0e9;
/// Pixels the sky reaches below the horizon beyond the plane's edge, so the row the horizon
/// runs through is sky at every sample rather than background.
const HORIZON_MARGIN_PIXELS: f64 = 1.5;

/// The horizon as a line on screen, in pixels with y up, with the unit normal pointing into
/// the sky.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HorizonLine {
    /// A point on the line.
    pub point: Point2<f64>,
    /// Unit normal pointing into the sky.
    pub normal: Vector2<f64>,
}

impl HorizonLine {
    /// Signed distance of a screen point from the horizon, positive in the sky.
    pub fn sky_distance(&self, point: Point2<f64>) -> f64 {
        (point - self.point).dot(self.normal)
    }

    /// The line as the sky and background shaders take it: point x, point y, normal x,
    /// normal y.
    pub fn to_shader(self) -> [f32; 4] {
        [
            self.point.x as f32,
            self.point.y as f32,
            self.normal.x as f32,
            self.normal.y as f32,
        ]
    }

    /// A horizon out of sight: the whole frame is ground, or all sky.
    fn out_of_sight(all_sky: bool) -> Self {
        let y = if all_sky {
            -OUT_OF_SIGHT_PIXELS
        } else {
            OUT_OF_SIGHT_PIXELS
        };
        Self {
            point: Point2::new(0.0, y),
            normal: Vector2::unit_y(),
        }
    }
}

impl ViewState {
    /// The horizon on screen.
    ///
    /// With the map's own camera it sits where GL JS `getMercatorHorizon` puts it, turned
    /// with the roll. With an external eye it is the image of the ground plane's line at
    /// infinity through the eye's own matrices, so it follows the eye wherever it looks
    /// rather than the pose the map keeps for its bookkeeping.
    pub fn horizon_line(&self) -> HorizonLine {
        if let Some(line) = self.eye_horizon_line() {
            return line;
        }
        let roll = self.camera.get_roll().0;
        let horizon = self.mercator_horizon();
        HorizonLine {
            point: Point2::new(
                self.width / 2.0 - horizon * roll.sin(),
                self.height / 2.0 + horizon * roll.cos(),
            ),
            normal: Vector2::new(-roll.sin(), roll.cos()),
        }
    }

    fn eye_horizon_line(&self) -> Option<HorizonLine> {
        self.external_eye?;
        let inverse = self.view_projection().0.invert()?;
        let eye_height = self.eye_position().z;
        // A ray's vertical component chooses the sky side even when the bookkeeping
        // center is behind the eye. Projecting that center would flip the gradient.
        let vertical = |clip: Vector4<f64>| {
            let point = inverse * clip;
            point.z - eye_height * point.w
        };
        let x = vertical(Vector4::new(1.0, 0.0, 0.0, 0.0));
        let y = vertical(Vector4::new(0.0, 1.0, 0.0, 0.0));
        let center = vertical(Vector4::new(0.0, 0.0, 0.0, 1.0));
        let normal = Vector2::new(2.0 * x / self.width, 2.0 * y / self.height);
        let offset = center - x - y;
        let length = normal.magnitude();
        if length < 1e-12 {
            // Looking straight down or up: the horizon is off every screen.
            return Some(HorizonLine::out_of_sight(center > 0.0));
        }
        let normal = normal / length;
        let offset = offset / length;
        // The Mercator plane ends at the poles' latitude. Between its nearest edge and the
        // horizon nothing is drawn, so the sky reaches down to cover that void, as the GL JS
        // horizon sits below the true one; the ground is drawn over it where there is any.
        let overlap = self.plane_edge_overlap() + HORIZON_MARGIN_PIXELS;
        Some(HorizonLine {
            point: Point2::from_vec(-normal * (offset + overlap)),
            normal,
        })
    }

    /// Pixels between the horizon and the nearest edge of the Mercator plane as the eye sees
    /// them: the eye's height over the distance to the edge, times the focal length.
    fn plane_edge_overlap(&self) -> f64 {
        let Some(frustum) = self.external_frustum() else {
            return 0.0;
        };
        let world_size = crate::coords::TILE_SIZE * 2f64.powf(self.zoom().value());
        let eye = self.eye_position();
        let edge_distance = eye
            .x
            .min(world_size - eye.x)
            .min(eye.y)
            .min(world_size - eye.y)
            .max(1.0);
        let height = eye.z.max(0.0) * self.pixels_per_meter();
        let focal_length = self.height / (frustum.top + frustum.bottom).max(1e-9);
        focal_length * height / edge_distance
    }
}

#[cfg(test)]
mod tests;
