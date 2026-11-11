//! Horizon, fog and near/far clip-plane depth calculations for [`ViewState`].

use std::f64::consts;

use cgmath::{Point2, Rad, Vector2};

use super::{NavigationMode, ViewState};
use crate::coords::TILE_SIZE;

/// Pitch beyond which the Mercator plane has no usable horizon.
pub(super) const MAX_MERCATOR_HORIZON_ANGLE: Rad<f64> = Rad(89.25 * consts::PI / 180.0);
/// Keeps some scene below the camera renderable when terrain dips under it.
const MIN_RENDER_DISTANCE_BELOW_CAMERA_METERS: f64 = 100.0;

impl ViewState {
    /// Distance in pixels from the screen center to the Mercator horizon, positive above the
    /// center, as GL JS `getMercatorHorizon`.
    pub fn mercator_horizon(&self) -> f64 {
        let pitch = self.camera.get_pitch().0.abs();
        self.camera_to_center_distance()
            * ((consts::FRAC_PI_2 - pitch).tan() * 0.85)
                .min((MAX_MERCATOR_HORIZON_ANGLE.0 - pitch).tan())
    }

    /// Returns a view shifted backward toward the camera's ground nadir when the camera is
    /// pitched (`> 20°`), covering the high-detail foreground strip immediately below the
    /// bottom edge of the screen so backward edge-scrolling never outruns tile loading.
    pub fn pitched_nadir_apron(&self) -> Option<Self> {
        if self.has_external_view() || self.navigation_mode() != NavigationMode::Constrained {
            return None;
        }
        let pitch_deg = self.camera.get_pitch().0.to_degrees().abs();
        if pitch_deg <= 20.0 {
            return None;
        }
        let center = self.camera.position();
        let eye = self.eye_position();
        let to_nadir = Vector2::new(eye.x - center.x, eye.y - center.y);
        if !to_nadir.x.is_finite()
            || !to_nadir.y.is_finite()
            || (to_nadir.x * to_nadir.x + to_nadir.y * to_nadir.y) < 1.0
        {
            return None;
        }
        let world_size = TILE_SIZE * 2_f64.powf(self.zoom.value());
        let mut apron = self.clone();
        let new_x = (center.x + to_nadir.x * 0.68).rem_euclid(world_size);
        let new_y = (center.y + to_nadir.y * 0.68).clamp(0.0, world_size);
        apron.camera.move_to(Point2::new(new_x, new_y));
        Some(apron)
    }

    /// Distances in pixels the fog depth spans: from the camera's distance to sea level, where
    /// the map center sits, to the far plane, as the GL JS fog matrix takes them. An external
    /// eye's fog is fixed by its height instead.
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
            .max(distance + self.center.elevation * self.pixels_per_meter() / limited_pitch.cos());
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
            .max(distance + self.center.elevation * pixels_per_meter / limited_pitch.cos());
        let camera_altitude = pitch.cos() * distance / pixels_per_meter + self.center.elevation;
        let min_elevation = self
            .center
            .elevation
            .min(self.min_elevation)
            .min(camera_altitude - MIN_RENDER_DISTANCE_BELOW_CAMERA_METERS);
        let lowest_plane = if min_elevation < 0.0 {
            camera_to_sea_level - min_elevation * pixels_per_meter / limited_pitch.cos()
        } else {
            camera_to_sea_level
        };

        let ground_angle = consts::FRAC_PI_2 + pitch;
        // A rolled view reaches further along the screen diagonal than the vertical field of
        // view alone, so the far plane widens the field of view with the roll as GL JS does.
        let roll = self.camera.get_roll().0;
        let rolled_fov = self.perspective.fovy().0
            * (roll.cos().abs() * self.height + roll.sin().abs() * self.width)
            / self.height;
        let fov_above_center = rolled_fov * (0.5 + center_offset.y / self.height);
        let surface_distance = |fov: f64| {
            fov.sin() * lowest_plane
                / (consts::PI - ground_angle - fov)
                    .clamp(0.01, consts::PI - 0.01)
                    .sin()
        };
        let top_half_surface_distance = surface_distance(fov_above_center);

        let horizon = distance
            * ((consts::FRAC_PI_2 - pitch).tan() * 0.85)
                .min((MAX_MERCATOR_HORIZON_ANGLE.0 - pitch).tan());
        let horizon_angle = (horizon / distance).atan();
        let min_fov_center_to_horizon = consts::FRAC_PI_2 - MAX_MERCATOR_HORIZON_ANGLE.0;
        let fov_center_to_horizon = if horizon_angle > min_fov_center_to_horizon {
            2.0 * horizon_angle * (0.5 + center_offset.y / (horizon * 2.0))
        } else {
            min_fov_center_to_horizon
        };
        let top_half_horizon_distance = surface_distance(fov_center_to_horizon);

        let top_half = top_half_surface_distance.min(top_half_horizon_distance);
        let far_z = ((consts::FRAC_PI_2 - limited_pitch).cos() * top_half + lowest_plane) * 1.01;
        let near_z = self.height / 50.0;
        (near_z, far_z)
    }
}
