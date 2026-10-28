//! The Earth's shadow where the moon crosses it, as a host computes it for a lunar eclipse.
//!
//! The shadow's size is a convention a host chooses, such as the enlargement an eclipse canon
//! applies, so the host gives its axis and radii rather than the renderer deriving them.

use cgmath::InnerSpace;

use super::{CelestialDiscs, CelestialError, EcefDirection};
use crate::tcs::world::World;

/// Widest angular radius the shadow's penumbra may have.
const MAX_RADIUS_DEGREES: f64 = 5.0;
/// Danjon's scale runs from L0, a moon almost lost in totality, to L4, a bright copper one.
const MAX_DANJON: f32 = 4.0;
/// A moon of middling darkness, deep red or rust, when the host gives no Danjon value.
const DEFAULT_DANJON: f32 = 2.0;

/// The Earth's shadow at the moon's distance, as seen from the eye: its axis and the angular
/// radii of its umbra and penumbra, and how dark the umbra is on Danjon's scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EarthShadow {
    axis: EcefDirection,
    umbra_degrees: f64,
    penumbra_degrees: f64,
    danjon: f32,
}

impl EarthShadow {
    /// The shadow along `axis`, from the eye towards the shadow's centre at the moon's distance,
    /// with an umbra smaller than its penumbra, both above zero and at most five degrees, and a
    /// Danjon value in 0..=4, L2 when none is given.
    pub fn new(
        axis: EcefDirection,
        umbra_radius_degrees: f64,
        penumbra_radius_degrees: f64,
        danjon: Option<f32>,
    ) -> Result<Self, CelestialError> {
        let danjon_value = danjon.unwrap_or(DEFAULT_DANJON);
        let nested = umbra_radius_degrees > 0.0
            && umbra_radius_degrees < penumbra_radius_degrees
            && penumbra_radius_degrees <= MAX_RADIUS_DEGREES;
        if nested && (0.0..=MAX_DANJON).contains(&danjon_value) {
            Ok(Self {
                axis,
                umbra_degrees: umbra_radius_degrees,
                penumbra_degrees: penumbra_radius_degrees,
                danjon: danjon_value,
            })
        } else {
            Err(CelestialError::InvalidEarthShadow {
                umbra_radius_degrees,
                penumbra_radius_degrees,
                danjon,
            })
        }
    }

    /// Direction from the eye to the shadow's centre.
    pub fn axis(&self) -> EcefDirection {
        self.axis
    }

    /// Angular radius of the umbra, in degrees.
    pub fn umbra_radius_degrees(&self) -> f64 {
        self.umbra_degrees
    }

    /// Angular radius of the penumbra, in degrees.
    pub fn penumbra_radius_degrees(&self) -> f64 {
        self.penumbra_degrees
    }

    /// How dark the umbra is, from 0 to 4 on Danjon's scale.
    pub fn danjon(&self) -> f32 {
        self.danjon
    }

    /// The share of the sun's light that reaches a point `direction` from the eye: none in the
    /// umbra, all beyond the penumbra, and a smooth ramp between, as the sun's disc emerges
    /// from behind the Earth.
    pub fn sunlit_share(&self, direction: EcefDirection) -> f64 {
        let angle = direction
            .ecef()
            .cross(self.axis.ecef())
            .magnitude()
            .atan2(direction.ecef().dot(self.axis.ecef()))
            .to_degrees();
        let t = ((angle - self.umbra_degrees) / (self.penumbra_degrees - self.umbra_degrees))
            .clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
}

/// Sets or clears the Earth's shadow the moon is lit through.
pub fn set_earth_shadow(world: &mut World, shadow: Option<EarthShadow>) {
    world
        .resources
        .get_or_init_mut::<CelestialDiscs>()
        .earth_shadow = shadow;
}
