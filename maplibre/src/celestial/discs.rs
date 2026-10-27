//! The discs of bodies near enough to show a size, such as the sun: where they stand and how
//! wide they look.

use super::{CelestialError, EcefDirection};
use crate::tcs::world::World;

/// Widest angular radius a disc may have; a body wider than this is not a disc in the sky.
const MAX_ANGULAR_RADIUS_DEGREES: f64 = 5.0;

/// A body's disc: the direction to its centre and its angular radius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Disc {
    direction: EcefDirection,
    angular_radius_degrees: f64,
}

impl Disc {
    /// The disc centred on `direction` with an angular radius in degrees, above zero and at
    /// most five.
    pub fn new(
        direction: EcefDirection,
        angular_radius_degrees: f64,
    ) -> Result<Self, CelestialError> {
        if angular_radius_degrees > 0.0 && angular_radius_degrees <= MAX_ANGULAR_RADIUS_DEGREES {
            Ok(Self {
                direction,
                angular_radius_degrees,
            })
        } else {
            Err(CelestialError::InvalidDisc {
                angular_radius_degrees,
            })
        }
    }

    /// Direction to the disc's centre.
    pub fn direction(&self) -> EcefDirection {
        self.direction
    }

    /// Angular radius in degrees.
    pub fn angular_radius_degrees(&self) -> f64 {
        self.angular_radius_degrees
    }
}

/// The discs the host set.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CelestialDiscs {
    /// The sun's disc, drawn with its glare.
    pub sun: Option<Disc>,
}

/// The discs the host set; none until it sets one.
pub fn discs(world: &World) -> CelestialDiscs {
    world
        .resources
        .get::<CelestialDiscs>()
        .copied()
        .unwrap_or_default()
}

/// Sets or clears the sun's disc. Its direction is the host's to keep in step with the sun
/// direction that lights the scene.
pub fn set_sun_disc(world: &mut World, sun: Option<Disc>) {
    world.resources.get_or_init_mut::<CelestialDiscs>().sun = sun;
}
