//! Camera zoom and geographic bounds constraints, plus `camera_for_bounds` / `fit_bounds`.

use cgmath::{Deg, Point2, Rad};

use super::{
    pose::{lat_lon_at_mercator, mercator_from_lat_lon},
    Camera, CameraOptions, NavigationError, ViewState,
};
use crate::{
    coords::{LatLon, Zoom, TILE_SIZE},
    projection::globe::scale::MERCATOR_LATITUDE_LIMIT,
};

#[inline]
fn clamp_ordered(value: f64, a: f64, b: f64) -> f64 {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    value.max(lo).min(hi)
}

#[inline]
fn clean_padding(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

/// Geographic bounding box in degrees (`west`/`east` longitudes, `south`/`north` latitudes).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LatLonBounds {
    /// Western longitude bound in degrees.
    pub west: f64,
    /// Southern latitude bound in degrees.
    pub south: f64,
    /// Eastern longitude bound in degrees.
    pub east: f64,
    /// Northern latitude bound in degrees.
    pub north: f64,
}

impl LatLonBounds {
    /// Creates a geographic bounding box from `(west, south, east, north)` in degrees.
    pub const fn new(west: f64, south: f64, east: f64, north: f64) -> Self {
        Self {
            west,
            south,
            east,
            north,
        }
    }

    /// Creates a geographic bounding box from southwest and northeast corners.
    pub const fn from_corners(south_west: LatLon, north_east: LatLon) -> Self {
        Self {
            west: south_west.longitude,
            south: south_west.latitude,
            east: north_east.longitude,
            north: north_east.latitude,
        }
    }

    /// Returns the southwest corner of this bounding box.
    pub fn south_west(&self) -> LatLon {
        LatLon::new(self.south, self.west)
    }

    /// Returns the northeast corner of this bounding box.
    pub fn north_east(&self) -> LatLon {
        LatLon::new(self.north, self.east)
    }

    /// Whether `point` lies inside or on the boundary of this box.
    pub fn contains(&self, point: LatLon) -> bool {
        let (lat_lo, lat_hi) = if self.south <= self.north {
            (self.south, self.north)
        } else {
            (self.north, self.south)
        };
        if point.latitude < lat_lo || point.latitude > lat_hi {
            return false;
        }
        if self.west <= self.east {
            point.longitude >= self.west && point.longitude <= self.east
        } else {
            point.longitude >= self.west || point.longitude <= self.east
        }
    }

    /// Clamps `point` to lie within this geographic bounding box and Web Mercator latitude limits.
    pub fn clamp(&self, point: LatLon) -> LatLon {
        let south = clamp_ordered(
            self.south.min(self.north),
            -MERCATOR_LATITUDE_LIMIT,
            MERCATOR_LATITUDE_LIMIT,
        );
        let north = clamp_ordered(
            self.south.max(self.north),
            -MERCATOR_LATITUDE_LIMIT,
            MERCATOR_LATITUDE_LIMIT,
        );
        let lat = clamp_ordered(point.latitude, south, north);
        let lon = if self.west <= self.east {
            clamp_ordered(point.longitude, self.west, self.east)
        } else if point.longitude >= self.west || point.longitude <= self.east {
            point.longitude
        } else {
            let dist_west = (self.west - point.longitude).abs();
            let dist_east = (point.longitude - self.east).abs();
            if dist_west <= dist_east {
                self.west
            } else {
                self.east
            }
        };
        LatLon::new(lat, lon)
    }

    /// Unit-Mercator `(min, max)` bounds for clamping `Camera` positions (`y` increases southward).
    pub(crate) fn to_mercator_range(self) -> (Point2<f64>, Point2<f64>) {
        let south = clamp_ordered(
            self.south.min(self.north),
            -MERCATOR_LATITUDE_LIMIT,
            MERCATOR_LATITUDE_LIMIT,
        );
        let north = clamp_ordered(
            self.south.max(self.north),
            -MERCATOR_LATITUDE_LIMIT,
            MERCATOR_LATITUDE_LIMIT,
        );
        let nw = mercator_from_lat_lon(LatLon::new(north, self.west));
        let se = mercator_from_lat_lon(LatLon::new(south, self.east));
        let (min_x, max_x) = if self.west <= self.east {
            (nw.x.min(se.x), nw.x.max(se.x))
        } else {
            (nw.x, se.x + 1.0)
        };
        let (min_y, max_y) = (nw.y.min(se.y), nw.y.max(se.y));
        (Point2::new(min_x, min_y), Point2::new(max_x, max_y))
    }
}

/// Per-edge viewport padding in logical pixels used when fitting camera bounds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EdgePadding {
    /// Padding from the top viewport edge in pixels.
    pub top: f64,
    /// Padding from the right viewport edge in pixels.
    pub right: f64,
    /// Padding from the bottom viewport edge in pixels.
    pub bottom: f64,
    /// Padding from the left viewport edge in pixels.
    pub left: f64,
}

impl EdgePadding {
    /// Creates per-edge padding from `(top, right, bottom, left)` in pixels.
    pub const fn new(top: f64, right: f64, bottom: f64, left: f64) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }

    /// Creates equal padding on all four edges.
    pub const fn uniform(padding: f64) -> Self {
        Self {
            top: padding,
            right: padding,
            bottom: padding,
            left: padding,
        }
    }

    /// Creates symmetric vertical (`top`/`bottom`) and horizontal (`left`/`right`) padding.
    pub const fn symmetric(vertical: f64, horizontal: f64) -> Self {
        Self {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }
}

/// Zoom and geographic center constraints enforced on a [`ViewState`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CameraBounds {
    /// Optional minimum continuous zoom level.
    pub min_zoom: Option<f64>,
    /// Optional maximum continuous zoom level.
    pub max_zoom: Option<f64>,
    /// Optional geographic bounding box restricting the camera's center.
    pub max_bounds: Option<LatLonBounds>,
}

/// Camera placement computed by [`ViewState::camera_for_bounds`] to frame a [`LatLonBounds`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraFit {
    /// Center coordinate that places the bounds in the center of the padded viewport.
    pub center: LatLon,
    /// Maximum continuous zoom level at which the bounds fit inside the padded viewport.
    pub zoom: f64,
    /// Bearing used when evaluating the fit.
    pub bearing: Deg<f64>,
    /// Pitch used when evaluating the fit.
    pub pitch: Deg<f64>,
}

impl From<CameraFit> for CameraOptions {
    fn from(fit: CameraFit) -> Self {
        Self {
            center: Some(fit.center),
            zoom: Some(fit.zoom),
            bearing: Some(fit.bearing),
            pitch: Some(fit.pitch),
            roll: None,
            elevation: None,
        }
    }
}

impl ViewState {
    /// Returns the active zoom and center bounds configured on this view.
    pub fn camera_bounds(&self) -> CameraBounds {
        self.bounds
    }

    /// Replaces the zoom and geographic center bounds and immediately clamps the current camera.
    pub fn set_camera_bounds(&mut self, bounds: CameraBounds) {
        self.bounds = CameraBounds {
            min_zoom: bounds.min_zoom.filter(|v| v.is_finite()),
            max_zoom: bounds.max_zoom.filter(|v| v.is_finite()),
            max_bounds: bounds.max_bounds,
        };
        self.apply_camera_bounds();
    }

    /// Sets or clears the minimum continuous zoom level and clamps the current zoom.
    pub fn set_min_zoom(&mut self, min_zoom: Option<f64>) {
        self.bounds.min_zoom = min_zoom.filter(|v| v.is_finite());
        self.apply_camera_bounds();
    }

    /// Returns the configured minimum continuous zoom level, if any.
    pub fn min_zoom(&self) -> Option<f64> {
        self.bounds.min_zoom
    }

    /// Sets or clears the maximum continuous zoom level and clamps the current zoom.
    pub fn set_max_zoom(&mut self, max_zoom: Option<f64>) {
        self.bounds.max_zoom = max_zoom.filter(|v| v.is_finite());
        self.apply_camera_bounds();
    }

    /// Returns the configured maximum continuous zoom level, if any.
    pub fn max_zoom(&self) -> Option<f64> {
        self.bounds.max_zoom
    }

    /// Sets or clears the geographic bounding box restricting the camera's center.
    pub fn set_max_bounds(&mut self, max_bounds: Option<LatLonBounds>) {
        self.bounds.max_bounds = max_bounds;
        self.apply_camera_bounds();
    }

    /// Returns the configured geographic center bounding box, if any.
    pub fn max_bounds(&self) -> Option<LatLonBounds> {
        self.bounds.max_bounds
    }

    /// Clamps `zoom` to [`Self::min_zoom`] and [`Self::max_zoom`].
    pub fn clamp_zoom(&self, zoom: Zoom) -> Zoom {
        let mut value = zoom.value();
        match (self.bounds.min_zoom, self.bounds.max_zoom) {
            (Some(min_z), Some(max_z)) => {
                value = clamp_ordered(value, min_z, max_z);
            }
            (Some(min_z), None) => {
                value = value.max(min_z);
            }
            (None, Some(max_z)) => {
                value = value.min(max_z);
            }
            (None, None) => {}
        }
        Zoom::new(value)
    }

    /// Clamps `center` to [`Self::max_bounds`] when configured.
    pub fn clamp_center(&self, center: LatLon) -> LatLon {
        match self.bounds.max_bounds {
            Some(bounds) => bounds.clamp(center),
            None => center,
        }
    }

    /// Sets the continuous zoom while keeping the geographic center, clamping to [`CameraBounds`].
    pub fn set_zoom(&mut self, zoom: Zoom) {
        self.zoom_to(zoom);
    }

    /// Applies `update` to the orbit camera while enforcing pitch and [`CameraBounds`] constraints.
    pub fn update_camera(&mut self, update: impl FnOnce(&mut Camera)) {
        update(self.camera_mut());
        let clamped_pos = self.camera().position();
        self.camera_mut().move_to(clamped_pos);
    }

    pub(super) fn apply_camera_bounds(&mut self) {
        let mercator_range = self.bounds.max_bounds.map(LatLonBounds::to_mercator_range);
        self.camera.set_mercator_bounds(mercator_range);
        let current_zoom = self.zoom();
        let clamped_zoom = self.clamp_zoom(current_zoom);
        if (clamped_zoom.value() - current_zoom.value()).abs() > f64::EPSILON {
            self.zoom_to(clamped_zoom);
        } else {
            let world_size = TILE_SIZE * 2.0_f64.powf(clamped_zoom.value());
            self.camera.set_world_size(world_size);
        }
    }

    /// Computes the center and maximum zoom that fit `bounds` inside the viewport padded by
    /// `padding`, accounting for `bearing` rotation and `pitch` perspective tilt.
    pub fn camera_for_bounds(
        &self,
        bounds: LatLonBounds,
        padding: EdgePadding,
        bearing: Option<Deg<f64>>,
        pitch: Option<Deg<f64>>,
    ) -> CameraFit {
        let pad_top = clean_padding(padding.top);
        let pad_right = clean_padding(padding.right);
        let pad_bottom = clean_padding(padding.bottom);
        let pad_left = clean_padding(padding.left);

        let avail_w = (self.width - pad_left - pad_right).max(1.0);
        let avail_h = (self.height - pad_top - pad_bottom).max(1.0);

        let bearing_deg = bearing.unwrap_or_else(|| self.camera().get_bearing().into());
        let max_pitch_deg = self.camera().max_pitch().0.to_degrees();
        let raw_pitch_deg = pitch.unwrap_or_else(|| self.camera().get_pitch().into()).0;
        let pitch_deg = Deg(clamp_ordered(raw_pitch_deg, -30.0, max_pitch_deg));

        let (min_m, max_m) = bounds.to_mercator_range();
        let mid_m = Point2::new((min_m.x + max_m.x) * 0.5, (min_m.y + max_m.y) * 0.5);
        let half_w_m = (max_m.x - min_m.x) * 0.5;
        let half_h_m = (max_m.y - min_m.y) * 0.5;

        let b = Rad::from(bearing_deg).0;
        let p = Rad::from(pitch_deg).0;
        let (sin_b, cos_b) = b.sin_cos();
        let (sin_p, cos_p) = p.sin_cos();

        let d_cam = self.camera_to_center_distance().max(1.0);
        let half_avail_w = avail_w * 0.5;
        let half_avail_h = avail_h * 0.5;

        let corners = [
            (-half_w_m, -half_h_m),
            (half_w_m, -half_h_m),
            (half_w_m, half_h_m),
            (-half_w_m, half_h_m),
        ];

        let mut max_world_size = f64::INFINITY;
        for (dx, dy) in corners {
            let sx_m = dx * cos_b + dy * sin_b;
            let sy_m = -dx * sin_b + dy * cos_b;

            let denom_x = sx_m.abs() * d_cam + half_avail_w * sy_m * sin_p;
            if denom_x > 1e-15 {
                max_world_size = max_world_size.min(half_avail_w * d_cam / denom_x);
            }
            let denom_y = sy_m.abs() * cos_p * d_cam + half_avail_h * sy_m * sin_p;
            if denom_y > 1e-15 {
                max_world_size = max_world_size.min(half_avail_h * d_cam / denom_y);
            }
        }

        let unclamped_zoom = if max_world_size.is_finite() && max_world_size > 0.0 {
            (max_world_size / TILE_SIZE).log2().clamp(-2.0, 24.0)
        } else {
            self.bounds
                .max_zoom
                .unwrap_or_else(|| self.zoom().value().clamp(-2.0, 24.0))
        };
        let zoom = self.clamp_zoom(Zoom::new(unclamped_zoom)).value();
        let world_size = TILE_SIZE * 2.0_f64.powf(zoom);

        // Shift the orbit center so `mid_m` lands at the center of the padded box rather than the
        // unpadded viewport center, accounting for perspective foreshortening and bearing.
        let center_offset = self.center_offset();
        let ox = (pad_left - pad_right) * 0.5 - center_offset.x;
        let oy = (pad_top - pad_bottom) * 0.5 - center_offset.y;
        let perspective_k = (cos_p + (oy / d_cam) * sin_p).max(0.05);
        let gx = ox * cos_p / perspective_k;
        let gy = oy / perspective_k;

        let shift_m_x = (gx * cos_b - gy * sin_b) / world_size;
        let shift_m_y = (gx * sin_b + gy * cos_b) / world_size;
        let cam_m = Point2::new(
            (mid_m.x - shift_m_x).rem_euclid(1.0),
            clamp_ordered(mid_m.y - shift_m_y, 0.0, 1.0),
        );
        let center = self.clamp_center(lat_lon_at_mercator(cam_m));

        CameraFit {
            center,
            zoom,
            bearing: bearing_deg,
            pitch: pitch_deg,
        }
    }

    /// Moves the camera immediately to frame `bounds` within `padding`, preserving the current
    /// bearing and pitch.
    pub fn fit_bounds(
        &mut self,
        bounds: LatLonBounds,
        padding: EdgePadding,
    ) -> Result<CameraFit, NavigationError> {
        let fit = self.camera_for_bounds(bounds, padding, None, None);
        self.jump_to(fit.into())?;
        Ok(fit)
    }
}

#[cfg(test)]
mod tests;
