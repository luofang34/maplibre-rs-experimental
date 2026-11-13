//! Drag and zoom on a free-globe camera, which turn the camera about the body's center and so
//! carry it over a pole as anywhere else.

use cgmath::{InnerSpace, Point2, Vector3};

use super::{turn_between, ViewState};
use crate::{
    projection::globe::{interaction::pan_surface_location, lat_lon_to_unit_sphere},
    render::projection::globe_camera_for_view,
};

/// Corrections that bring a zoom's anchor back under its pixel.
const ANCHOR_PASSES: usize = 6;

/// Times a zoom step that leaves no drawable camera is halved before the zoom stops, which
/// brings it within a thousandth of the step of the last distance a camera can be drawn from.
const ZOOM_HALVINGS: usize = 10;

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
    /// whole camera about the body's center. When `from` or `to` lies outside the visible limb,
    /// translates the drag onto the virtual trackball anchored at `from` (or at the globe's
    /// screen center when starting in empty space), matching GL JS / ForeFlight whole-globe pan.
    /// Returns whether it moved.
    pub fn drag_free_globe(&mut self, from: Point2<f64>, to: Point2<f64>) -> bool {
        if self.navigation_mode() != super::NavigationMode::FreeGlobe {
            return false;
        }
        let Ok(camera) = globe_camera_for_view(self) else {
            return false;
        };
        let (grabbed, under) =
            if camera.is_point_on_map_surface(from) && camera.is_point_on_map_surface(to) {
                let (Some(grabbed), Some(under)) = (self.grabbed(from), self.grabbed(to)) else {
                    return false;
                };
                (grabbed, under)
            } else {
                let center_px = camera.location_to_screen(camera.center(), self.center_elevation());
                let anchor_px = if camera.is_point_on_map_surface(from) {
                    from
                } else {
                    center_px
                };
                let target_px = anchor_px + (to - from);
                let (Some(grabbed_loc), Some(under_loc)) = (
                    pan_surface_location(&camera, anchor_px),
                    pan_surface_location(&camera, target_px),
                ) else {
                    return false;
                };
                (
                    lat_lon_to_unit_sphere(grabbed_loc),
                    lat_lon_to_unit_sphere(under_loc),
                )
            };
        // The camera sees a point turned by the opposite of its own turn; the point under `to`
        // must turn onto the grabbed one. Eye and target turn together about the body's center,
        // keeping their heights, so a camera that could be drawn still can.
        self.turn_free_globe(turn_between(under, grabbed));
        true
    }

    /// Zooms the free-globe camera by `zoom_delta` keeping the ground under `pixel` there, or
    /// by as much of it as leaves a camera that can be drawn, as one looking up at a raised
    /// target cannot once zooming out takes its eye under the ground. Returns whether it moved.
    pub fn zoom_free_globe(&mut self, pixel: Point2<f64>, zoom_delta: f64) -> bool {
        if self.navigation_mode() != super::NavigationMode::FreeGlobe {
            return false;
        }
        let Some(anchor) = self.grabbed(pixel) else {
            return false;
        };
        let mut delta = zoom_delta;
        for _ in 0..=ZOOM_HALVINGS {
            let before = self.clone();
            self.zoom_free_globe_about(anchor, pixel, delta);
            if self.keep_if_drawable(before) {
                return true;
            }
            delta *= 0.5;
        }
        false
    }

    /// Zooms by `zoom_delta`, then turns the camera until `anchor` lies under `pixel` again.
    fn zoom_free_globe_about(&mut self, anchor: Vector3<f64>, pixel: Point2<f64>, zoom_delta: f64) {
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
    }

    /// Keeps a gesture's result if the globe camera can be drawn from it, the same rule
    /// [`ViewState::set_globe_pose`] holds a placed pose to; otherwise puts back `before`.
    pub(super) fn keep_if_drawable(&mut self, before: ViewState) -> bool {
        if globe_camera_for_view(self).is_ok() {
            true
        } else {
            *self = before;
            false
        }
    }
}
