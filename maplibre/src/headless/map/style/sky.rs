//! The sun, its light, the atmosphere and the stars a host sets on the offscreen map in place of the
//! style's `light` and `sky`, as [`crate::map::Map`] sets its own.

use crate::{
    celestial::{EcefDirection, TwilightBand},
    headless::map::HeadlessMap,
};

impl HeadlessMap {
    /// Sets the direction towards the sun in ECEF axes, as
    /// [`crate::context::MapContext::set_sun_direction`] does; `None` returns to the style's light.
    pub fn set_sun_direction(&mut self, sun: Option<EcefDirection>) {
        self.map_context.set_sun_direction(sun);
    }

    /// Sets the solar elevations over which the terrain turns from night to day, as
    /// [`crate::context::MapContext::set_twilight_band`] does.
    pub fn set_twilight_band(&mut self, twilight: TwilightBand) {
        self.map_context.set_twilight_band(twilight);
    }

    /// Replaces the stars drawn behind the globe, as
    /// [`crate::context::MapContext::set_stars`] does.
    pub fn set_stars(&mut self, stars: Vec<crate::celestial::Star>) {
        self.map_context.set_stars(stars);
    }

    /// Sets or clears the sun's disc, as [`crate::context::MapContext::set_sun_disc`] does.
    pub fn set_sun_disc(&mut self, sun: Option<crate::celestial::Disc>) {
        self.map_context.set_sun_disc(sun);
    }

    /// Draws the globe's atmosphere without a style `sky`, as
    /// [`crate::context::MapContext::set_atmosphere`] does.
    pub fn set_atmosphere(&mut self, atmosphere: bool) {
        self.map_context.set_atmosphere(atmosphere);
    }
}

#[cfg(test)]
mod daylight;
#[cfg(test)]
mod host_atmosphere;
#[cfg(test)]
mod starfield;
#[cfg(test)]
mod sun;
