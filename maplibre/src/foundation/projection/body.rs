//! The body the map is drawn on. Earth by default; the radius is the figure the globe
//! projection, terrain and the atmosphere need, and the height of the highest ground bounds
//! terrain no elevation data has described yet, so a second body only changes them here.

/// A spherical body the map projects onto.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    /// Mean radius in metres.
    pub radius_meters: f64,
    /// Height in metres of the body's highest ground above the mean radius: where no
    /// elevation data has loaded, ground anywhere below it may hide what lies beyond.
    pub highest_ground_meters: f64,
    /// Height in metres of the body's lowest ground relative to the mean radius (negative for
    /// sub-sea-level basins such as the Dead Sea or Death Valley).
    pub lowest_ground_meters: f64,
}

impl Body {
    /// Earth, with the mean radius GL JS uses.
    pub const EARTH: Self = Self {
        radius_meters: 6_371_008.8,
        highest_ground_meters: 8_849.0,
        lowest_ground_meters: -450.0,
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
mod tests {
    use super::Body;

    #[test]
    fn earth_is_the_default_and_a_smaller_body_scales_every_derived_length() {
        let moon = Body {
            radius_meters: 1_737_400.0,
            highest_ground_meters: 10_786.0,
            lowest_ground_meters: -9_129.0,
        };

        assert_eq!(Body::default(), Body::EARTH);
        assert_eq!(Body::EARTH.lowest_ground_meters, -450.0);
        assert!((Body::EARTH.circumference_meters() - 40_030_228.9).abs() < 1.0);
        assert!(
            (moon.circumference_at_latitude(60.0) - moon.circumference_meters() * 0.5).abs() < 1e-6
        );
        assert!((moon.unit_radius_at(1_737.4) - 1.001).abs() < 1e-9);
    }
}
