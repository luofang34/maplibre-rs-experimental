//! Drag and zoom on a free-globe camera, which turn the camera about the body's center and so
//! carry it over a pole as anywhere else.

use cgmath::{InnerSpace, Point2, Vector3};

use super::{turn_between, ViewState};
use crate::{projection::globe::lat_lon_to_unit_sphere, render::projection::globe_camera_for_view};

/// Corrections that bring a zoom's anchor back under its pixel.
const ANCHOR_PASSES: usize = 6;

impl ViewState {
    /// The point of the sphere at the center's elevation under `pixel`, or on its horizon where
    /// the pixel's ray passes it, in the body's frame.
    fn grabbed(&self, pixel: Point2<f64>) -> Option<Vector3<f64>> {
        let camera = globe_camera_for_view(self).ok()?;
        let elevation = self.center_elevation();
        let location = camera
            .screen_point_to_location_at(pixel, elevation)
            .or_else(|| camera.screen_point_to_location(pixel))?;
        Some(lat_lon_to_unit_sphere(location))
    }

    /// Drags the free-globe camera so the ground under `from` moves under `to`, turning the
    /// whole camera about the body's center. Returns whether it moved.
    pub fn drag_free_globe(&mut self, from: Point2<f64>, to: Point2<f64>) -> bool {
        if self.navigation_mode() != super::NavigationMode::FreeGlobe {
            return false;
        }
        let (Some(grabbed), Some(under)) = (self.grabbed(from), self.grabbed(to)) else {
            return false;
        };
        // The camera sees a point turned by the opposite of its own turn; the point under `to`
        // must turn onto the grabbed one.
        self.turn_free_globe(turn_between(under, grabbed));
        true
    }

    /// Zooms the free-globe camera by `zoom_delta` keeping the ground under `pixel` there.
    /// Returns whether it moved.
    pub fn zoom_free_globe(&mut self, pixel: Point2<f64>, zoom_delta: f64) -> bool {
        if self.navigation_mode() != super::NavigationMode::FreeGlobe {
            return false;
        }
        let Some(anchor) = self.grabbed(pixel) else {
            return false;
        };
        self.scale_free_globe_distance(2_f64.powf(-zoom_delta));
        for _ in 0..ANCHOR_PASSES {
            let Some(under) = self.grabbed(pixel) else {
                break;
            };
            if (under - anchor).magnitude() < 1e-15 {
                break;
            }
            self.turn_free_globe(turn_between(under, anchor));
        }
        true
    }
}
