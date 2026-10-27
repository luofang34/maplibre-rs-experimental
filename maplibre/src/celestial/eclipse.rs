//! What an eclipse takes from the light: the share of the sun's disc the moon leaves uncovered.

use super::{CelestialError, CelestialLighting};
use crate::tcs::world::World;

/// The share of the sun's disc left uncovered where the eye stands, from 0 in totality to 1
/// outside an eclipse, in linear light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VisibleFraction(f32);

impl Default for VisibleFraction {
    /// The whole disc, as outside an eclipse.
    fn default() -> Self {
        Self(1.0)
    }
}

impl VisibleFraction {
    /// The share `fraction`, in 0..=1.
    pub fn new(fraction: f32) -> Result<Self, CelestialError> {
        if (0.0..=1.0).contains(&fraction) {
            Ok(Self(fraction))
        } else {
            Err(CelestialError::InvalidVisibleFraction { fraction })
        }
    }

    /// The share in 0..=1.
    pub fn get(self) -> f32 {
        self.0
    }
}

/// Sets the share of the sun's disc left uncovered where the eye stands. The sky's scattered
/// sunlight and the day side's light scale with it; the sun's disc and its glare do not, as the
/// moon's disc covers the one and the other stands in for the corona.
pub fn set_sun_visible_fraction(world: &mut World, fraction: VisibleFraction) {
    world
        .resources
        .get_or_init_mut::<CelestialLighting>()
        .sun_visible = fraction;
}
