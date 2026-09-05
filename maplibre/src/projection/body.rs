//! The body the map is drawn on. Earth by default; the radius is the one figure the globe
//! projection, terrain and the atmosphere need, so a second body only has to change it here.

/// A spherical body the map projects onto.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    /// Mean radius in metres.
    pub radius_meters: f64,
}

impl Body {
    /// Earth, with the mean radius GL JS uses.
    pub const EARTH: Self = Self {
        radius_meters: 6_371_008.8,
    };

    /// Metres around the equator, the length one Mercator unit covers there.
    pub fn circumference_meters(self) -> f64 {
        2.0 * std::f64::consts::PI * self.radius_meters
    }

    /// Metres around the parallel at a latitude.
    pub fn circumference_at_latitude(self, latitude_degrees: f64) -> f64 {
        self.circumference_meters() * latitude_degrees.to_radians().cos()
    }

    /// Radius of the unit sphere at an elevation in metres above the surface.
    pub fn unit_radius_at(self, elevation_meters: f64) -> f64 {
        1.0 + elevation_meters / self.radius_meters
    }
}

impl Default for Body {
    fn default() -> Self {
        Self::EARTH
    }
}

#[cfg(test)]
mod tests;
