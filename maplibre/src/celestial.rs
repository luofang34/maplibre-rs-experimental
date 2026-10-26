//! Sun and sky inputs a host supplies in Earth-centred, Earth-fixed axes.
//!
//! The renderer computes no astronomy: a host works out where the sun is and hands its
//! direction over as an [`EcefDirection`]. While none is set, the style's root `light` drives
//! the atmosphere as it does without this module.

use cgmath::{InnerSpace, Vector3};
use thiserror::Error;

use crate::tcs::world::World;

const MIN_LENGTH_SQUARED: f64 = 1e-24;

/// A direction refused as an [`EcefDirection`].
#[derive(Clone, Copy, Debug, Error, PartialEq)]
pub enum CelestialError {
    /// A component is NaN or infinite, or the vector is too short to have a direction.
    #[error("direction ({x}, {y}, {z}) is not a finite, non-zero vector")]
    InvalidDirection {
        /// ECEF x component as given.
        x: f64,
        /// ECEF y component as given.
        y: f64,
        /// ECEF z component as given.
        z: f64,
    },
}

/// A unit vector in ECEF (ITRF) axes: x towards 0°N 0°E, y towards 0°N 90°E, z towards the
/// north pole.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EcefDirection(Vector3<f64>);

impl EcefDirection {
    /// Normalizes the vector given, which must be finite and not zero.
    pub fn new(x: f64, y: f64, z: f64) -> Result<Self, CelestialError> {
        let vector = Vector3::new(x, y, z);
        let length_squared = vector.magnitude2();
        if length_squared.is_finite() && length_squared > MIN_LENGTH_SQUARED {
            Ok(Self(vector / length_squared.sqrt()))
        } else {
            Err(CelestialError::InvalidDirection { x, y, z })
        }
    }

    /// The unit vector in ECEF axes.
    pub fn ecef(self) -> Vector3<f64> {
        self.0
    }

    /// The unit vector in the axes the globe is drawn in; see [`ecef_to_globe_world`].
    pub fn globe_world(self) -> Vector3<f64> {
        ecef_to_globe_world(self.0)
    }
}

/// Turns ECEF axes into the globe's world axes, in which latitude and longitude zero lie on
/// +z, 90°E on +x and the north pole on +y, as
/// [`crate::projection::globe::lat_lon_to_unit_sphere`] places them.
pub fn ecef_to_globe_world(ecef: Vector3<f64>) -> Vector3<f64> {
    Vector3::new(ecef.y, ecef.z, ecef.x)
}

/// What the host has said about the sky, kept as a world resource for the render systems.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CelestialLighting {
    /// Direction from the Earth towards the sun; the style's light applies while it is unset.
    pub sun: Option<EcefDirection>,
}

/// The sun direction the host set, if any.
pub fn sun_direction(world: &World) -> Option<EcefDirection> {
    world
        .resources
        .get::<CelestialLighting>()
        .and_then(|lighting| lighting.sun)
}

/// Sets or clears the sun direction the render systems read.
pub fn set_sun_direction(world: &mut World, sun: Option<EcefDirection>) {
    world.resources.get_or_init_mut::<CelestialLighting>().sun = sun;
}

#[cfg(test)]
mod tests;
