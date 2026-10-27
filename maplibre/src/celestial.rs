//! Sun and sky inputs a host supplies in Earth-centred, Earth-fixed axes.
//!
//! The renderer computes no astronomy: a host works out where the sun and the stars are and
//! hands their directions over as [`EcefDirection`]s. While no sun is set, the style's root
//! `light` drives the atmosphere as it does without this module.

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
    /// The twilight band does not run upwards between finite elevations, or the night
    /// brightness lies outside 0..=1.
    #[error(
        "twilight from {night_end_degrees}° to {day_start_degrees}° with night brightness \
         {night_brightness} is not an ascending band of finite elevations with a brightness \
         in 0..=1"
    )]
    InvalidTwilight {
        /// Solar elevation given for the end of night, in degrees.
        night_end_degrees: f64,
        /// Solar elevation given for the start of full day, in degrees.
        day_start_degrees: f64,
        /// Brightness given for the night side.
        night_brightness: f32,
    },
    /// A star's magnitude is not finite, or its colour leaves 0..=1.
    #[error("star of magnitude {magnitude} and colour {color:?} cannot be drawn")]
    InvalidStar {
        /// Magnitude given.
        magnitude: f32,
        /// Colour given.
        color: Option<[f32; 3]>,
    },
    /// A disc's angular radius is not above zero and at most five degrees.
    #[error("disc of angular radius {angular_radius_degrees}° cannot be drawn")]
    InvalidDisc {
        /// Angular radius given, in degrees.
        angular_radius_degrees: f64,
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

/// The night side's share of the day's light by default.
pub const NIGHT_BRIGHTNESS: f32 = 0.35;
/// What a full Moon overhead adds to the night side's share of the day's light.
pub const MOONLIGHT_GAIN: f32 = 0.25;

/// The solar elevations over which the ground turns from night to day, and how bright the
/// night side stays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TwilightBand {
    night_end_degrees: f64,
    day_start_degrees: f64,
    night_brightness: f32,
}

impl Default for TwilightBand {
    /// Night ends at civil dusk, 6° below the horizon, and full day begins with the sun 2°
    /// above it. The night side keeps a third of the day's light, so its roads, labels and
    /// relief stay readable rather than turning black.
    fn default() -> Self {
        Self {
            night_end_degrees: -6.0,
            day_start_degrees: 2.0,
            night_brightness: NIGHT_BRIGHTNESS,
        }
    }
}

impl TwilightBand {
    /// A band from `night_end_degrees` to the higher `day_start_degrees` of solar elevation,
    /// with the night side at `night_brightness` of the day side's light.
    pub fn new(
        night_end_degrees: f64,
        day_start_degrees: f64,
        night_brightness: f32,
    ) -> Result<Self, CelestialError> {
        let ascending = night_end_degrees.is_finite()
            && day_start_degrees.is_finite()
            && (-90.0..day_start_degrees).contains(&night_end_degrees)
            && day_start_degrees <= 90.0;
        if ascending && (0.0..=1.0).contains(&night_brightness) {
            Ok(Self {
                night_end_degrees,
                day_start_degrees,
                night_brightness,
            })
        } else {
            Err(CelestialError::InvalidTwilight {
                night_end_degrees,
                day_start_degrees,
                night_brightness,
            })
        }
    }

    /// The sines of the band's two elevations and the night brightness, as the shaders compare
    /// them with the cosine between the surface normal and the sun.
    fn uniform(self) -> [f32; 4] {
        [
            self.night_end_degrees.to_radians().sin() as f32,
            self.day_start_degrees.to_radians().sin() as f32,
            self.night_brightness,
            0.0,
        ]
    }
}

/// What the host has said about the sky, kept as a world resource for the render systems.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CelestialLighting {
    /// Direction from the Earth towards the sun; the style's light applies while it is unset.
    pub sun: Option<EcefDirection>,
    /// Where the ground turns from night to day.
    pub twilight: TwilightBand,
    /// Whether the globe's atmosphere is drawn in full whatever the style's `sky` says.
    pub atmosphere: bool,
}

/// What lights the terrain: the host's sky and its Moon.
pub(crate) type TerrainLight = (CelestialLighting, Option<Disc>);

/// The Moon's light on the night side: its direction in globe world axes and, as w, the
/// light it adds overhead: [`MOONLIGHT_GAIN`] times its lit fraction (1 − cos elongation) / 2,
/// from the angle between the Sun and Moon directions. All zero without both bodies.
pub fn moonlight_uniform(sun: Option<EcefDirection>, moon: Option<Disc>) -> [f32; 4] {
    let (Some(sun), Some(moon)) = (sun, moon) else {
        return [0.0; 4];
    };
    let direction = moon.direction();
    let lit_fraction = (1.0 - sun.ecef().dot(direction.ecef())) / 2.0;
    let world = direction.globe_world();
    [
        world.x as f32,
        world.y as f32,
        world.z as f32,
        MOONLIGHT_GAIN * lit_fraction as f32,
    ]
}

impl CelestialLighting {
    /// The sun direction in globe world axes, with w 1 while a host has set one and all zero
    /// otherwise, followed by the twilight band, as the terrain shader takes them.
    pub fn daylight_uniforms(self) -> [[f32; 4]; 2] {
        let sun = self.sun.map_or([0.0; 4], |sun| {
            let world = sun.globe_world();
            [world.x as f32, world.y as f32, world.z as f32, 1.0]
        });
        [sun, self.twilight.uniform()]
    }
}

/// The host's sun and twilight band, or the defaults while it has set none.
pub fn lighting(world: &World) -> CelestialLighting {
    world
        .resources
        .get::<CelestialLighting>()
        .copied()
        .unwrap_or_default()
}

/// The sun direction the host set, if any.
pub fn sun_direction(world: &World) -> Option<EcefDirection> {
    world
        .resources
        .get::<CelestialLighting>()
        .and_then(|lighting| lighting.sun)
}

/// Sets the band over which the ground turns from night to day.
pub fn set_twilight_band(world: &mut World, twilight: TwilightBand) {
    world
        .resources
        .get_or_init_mut::<CelestialLighting>()
        .twilight = twilight;
}

/// Draws the globe's atmosphere without a style `sky`, or leaves it to the style.
pub fn set_atmosphere(world: &mut World, atmosphere: bool) {
    world
        .resources
        .get_or_init_mut::<CelestialLighting>()
        .atmosphere = atmosphere;
}

/// Sets or clears the sun direction the render systems read.
pub fn set_sun_direction(world: &mut World, sun: Option<EcefDirection>) {
    world.resources.get_or_init_mut::<CelestialLighting>().sun = sun;
}

mod discs;
mod stars;

pub use discs::{
    discs, set_moon_disc, set_sun_angular_radius, AngularRadius, CelestialDiscs, Disc,
};
pub use stars::{set_stars, Star, Starfield};

#[cfg(test)]
mod tests;
