//! The discs of bodies near enough to show a size: the sun, which stands where the sun direction
//! points, and bodies with directions of their own.

use super::{CelestialError, EcefDirection};
use crate::tcs::world::World;

/// Widest angular radius a disc may have; a body wider than this is not a disc in the sky.
const MAX_ANGULAR_RADIUS_DEGREES: f64 = 5.0;

/// How wide a disc looks: its angular radius in degrees, above zero and at most five.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AngularRadius(f64);

impl AngularRadius {
    /// The angular radius of `degrees`, above zero and at most five.
    pub fn new(degrees: f64) -> Result<Self, CelestialError> {
        if degrees > 0.0 && degrees <= MAX_ANGULAR_RADIUS_DEGREES {
            Ok(Self(degrees))
        } else {
            Err(CelestialError::InvalidDisc {
                angular_radius_degrees: degrees,
            })
        }
    }

    /// The radius in degrees.
    pub fn degrees(self) -> f64 {
        self.0
    }
}

/// A body's disc with a direction of its own: the direction to its centre and its radius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Disc {
    direction: EcefDirection,
    radius: AngularRadius,
}

impl Disc {
    /// The disc centred on `direction` with an angular radius in degrees, above zero and at
    /// most five.
    pub fn new(
        direction: EcefDirection,
        angular_radius_degrees: f64,
    ) -> Result<Self, CelestialError> {
        Ok(Self {
            direction,
            radius: AngularRadius::new(angular_radius_degrees)?,
        })
    }

    /// Direction to the disc's centre.
    pub fn direction(&self) -> EcefDirection {
        self.direction
    }

    /// Angular radius.
    pub fn radius(&self) -> AngularRadius {
        self.radius
    }
}

/// The discs the host set.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CelestialDiscs {
    /// The sun's radius. The disc stands where the sun direction points, so the disc and the
    /// light on the ground and in the sky cannot disagree; with no sun direction it is not drawn.
    pub sun_radius: Option<AngularRadius>,
}

/// The discs the host set; none until it sets one.
pub fn discs(world: &World) -> CelestialDiscs {
    world
        .resources
        .get::<CelestialDiscs>()
        .copied()
        .unwrap_or_default()
}

/// Sets or clears the sun's angular radius, which draws its disc along the sun direction.
pub fn set_sun_angular_radius(world: &mut World, radius: Option<AngularRadius>) {
    world
        .resources
        .get_or_init_mut::<CelestialDiscs>()
        .sun_radius = radius;
}
