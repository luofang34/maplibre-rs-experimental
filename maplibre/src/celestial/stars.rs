//! The stars a host supplies: directions, magnitudes and colours, with no astronomy inside.

use std::sync::Arc;

use super::{CelestialError, EcefDirection};
use crate::tcs::world::World;

/// One star as the host sees it from the Earth.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Star {
    direction: EcefDirection,
    magnitude: f32,
    color: Option<[f32; 3]>,
}

impl Star {
    /// A star in `direction` at visual `magnitude`, brighter as it falls, drawn in `color`, an
    /// sRGB triple in 0..=1, or white.
    pub fn new(
        direction: EcefDirection,
        magnitude: f32,
        color: Option<[f32; 3]>,
    ) -> Result<Self, CelestialError> {
        let color_valid =
            color.is_none_or(|color| color.iter().all(|channel| (0.0..=1.0).contains(channel)));
        if magnitude.is_finite() && color_valid {
            Ok(Self {
                direction,
                magnitude,
                color,
            })
        } else {
            Err(CelestialError::InvalidStar { magnitude, color })
        }
    }

    /// Direction from the Earth towards the star.
    pub fn direction(&self) -> EcefDirection {
        self.direction
    }

    /// Visual magnitude.
    pub fn magnitude(&self) -> f32 {
        self.magnitude
    }

    /// sRGB colour, white when the host gave none.
    pub fn color(&self) -> [f32; 3] {
        self.color.unwrap_or([1.0; 3])
    }
}

/// The stars the host set, with a generation that changes whenever they do, so the GPU copy
/// is rebuilt only then.
#[derive(Clone, Debug, Default)]
pub struct Starfield {
    stars: Arc<[Star]>,
    generation: u64,
}

impl Starfield {
    /// The stars, in the order the host gave them.
    pub fn stars(&self) -> &Arc<[Star]> {
        &self.stars
    }

    /// Changes each time the stars are set.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

/// Replaces the stars the starfield draws; an empty list draws none.
pub fn set_stars(world: &mut World, stars: Vec<Star>) {
    let starfield = world.resources.get_or_init_mut::<Starfield>();
    starfield.stars = stars.into();
    starfield.generation = starfield.generation.wrapping_add(1);
}
