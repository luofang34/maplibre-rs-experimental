//! The height of the point the camera orbits at the map center, and what decides it.
//!
//! With terrain the center rests on the ground under it while it is clamped there, as GL JS
//! `centerClampedToGround` keeps it; without terrain, or unclamped, it rests at the altitude
//! the style's `centerAltitude` or the host set. A gesture freezes it so the camera does not
//! bob with every change of the ground under the center.

use super::ViewState;

/// The center's elevation and the state that decides it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CenterElevation {
    /// Elevation in metres of the point the camera orbits, exaggeration included.
    pub(super) elevation: f64,
    /// Altitude in metres above sea level the center rests at off the ground.
    altitude: f64,
    /// Whether the center follows the terrain under it.
    clamped_to_ground: bool,
    /// Whether a gesture holds the elevation still, as GL JS `elevationFreeze` does.
    frozen: bool,
    /// Whether the globe camera orbits the center's elevation rather than sea level.
    globe_orbits: bool,
}

impl Default for CenterElevation {
    fn default() -> Self {
        Self {
            elevation: 0.0,
            altitude: 0.0,
            clamped_to_ground: true,
            frozen: false,
            globe_orbits: false,
        }
    }
}

impl ViewState {
    /// Elevation in metres of the point the camera orbits at the map center.
    pub fn center_elevation(&self) -> f64 {
        self.center.elevation
    }

    /// Sets the elevation of the point the camera orbits; the camera keeps its distance to it.
    pub fn set_center_elevation(&mut self, meters: f64) {
        if meters.is_finite() {
            self.center.elevation = meters;
        }
    }

    /// Altitude in metres above sea level the center rests at where the terrain does not hold
    /// it, as the style's `centerAltitude` gives it.
    pub fn center_altitude(&self) -> f64 {
        self.center.altitude
    }

    /// Rests the center at `meters` above sea level where the terrain does not hold it, and
    /// moves it there now.
    pub fn set_center_altitude(&mut self, meters: f64) {
        if meters.is_finite() {
            self.center.altitude = meters;
            self.center.elevation = meters;
        }
    }

    /// Whether the center follows the terrain under it, as GL JS `centerClampedToGround`.
    pub fn center_clamped_to_ground(&self) -> bool {
        self.center.clamped_to_ground
    }

    /// Makes the center follow the terrain under it, or rest at its altitude.
    pub fn set_center_clamped_to_ground(&mut self, clamped: bool) {
        self.center.clamped_to_ground = clamped;
    }

    /// Holds the center elevation still until [`thaw_center_elevation`](Self::thaw_center_elevation).
    ///
    /// The camera would otherwise bob with every change of the terrain under the center while a
    /// drag or zoom is in progress.
    pub fn freeze_center_elevation(&mut self) {
        self.center.frozen = true;
    }

    /// Lets the center elevation follow the terrain again.
    pub fn thaw_center_elevation(&mut self) {
        self.center.frozen = false;
    }

    /// Whether a gesture currently holds the center elevation still.
    pub fn center_elevation_frozen(&self) -> bool {
        self.center.frozen
    }

    /// Whether the globe camera orbits [`center_elevation`](Self::center_elevation), not sea level.
    pub fn globe_orbits_center(&self) -> bool {
        self.center.globe_orbits
    }

    /// Makes the globe camera orbit the center's elevation, or sea level.
    pub fn set_globe_orbits_center(&mut self, orbits: bool) {
        self.center.globe_orbits = orbits;
    }
}
